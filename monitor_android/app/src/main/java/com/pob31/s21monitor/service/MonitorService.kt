package com.pob31.s21monitor.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import com.pob31.s21monitor.R
import com.pob31.s21monitor.data.CredentialsStore
import com.pob31.s21monitor.model.AuxState
import com.pob31.s21monitor.model.Credentials
import com.pob31.s21monitor.model.LinkProblem
import com.pob31.s21monitor.model.MonitorUiState
import com.pob31.s21monitor.model.SendState
import com.pob31.s21monitor.net.chooseNetwork
import com.pob31.s21monitor.net.networkOptions
import com.pob31.s21monitor.osc.MonitorProtocol
import com.pob31.s21monitor.osc.MonitorProtocol.Inbound
import com.pob31.s21monitor.osc.OscArg
import com.pob31.s21monitor.osc.OscBool
import com.pob31.s21monitor.osc.OscCodec
import com.pob31.s21monitor.osc.OscFloat
import com.pob31.s21monitor.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * Foreground service that owns the UDP/OSC link to the daemon and keeps it
 * alive with the screen off / UI closed — the whole reason this native client
 * exists. Architecture mirrors the WFS-DIY remote's OscService: started +
 * bound, START_STICKY, a dedicated receive thread draining into a queue, a
 * coroutine processing loop, and state exposed as a [StateFlow] the Compose UI
 * collects directly (no IPC). Outbound packets go through an [OutboundQueue]
 * drained by one sender coroutine.
 *
 * One difference from WFS-DIY: the daemon replies to the *source* address of
 * our packets, so we send and receive on a single shared socket (bound to an
 * ephemeral port) — see [startNetworking].
 */
class MonitorService : Service() {

    private val binder = LocalBinder()
    private val job = SupervisorJob()

    private val _state = MutableStateFlow(MonitorUiState())
    val state: StateFlow<MonitorUiState> = _state.asStateFlow()

    private var creds: Credentials? = null
    private var socket: DatagramSocket? = null
    private var receiveThread: Thread? = null
    /** The current link's coroutines; cancelled by [stopLink]. */
    private var link: Job? = null

    /** Packets for the link's single sender, which sends them in order
     *  (audit A3). [wakeSender] tells it there is something to send. */
    private val outbound = OutboundQueue(RESEND_AFTER_MS)
    private val wakeSender = Channel<Unit>(Channel.CONFLATED)

    /** Controls in use, whose incoming values are ignored (audit A5). */
    private val holds = ControlHolds(HOLD_AFTER_RELEASE_MS)

    @Volatile private var running = false

    /** The daemon's address, once resolved. Packets from anywhere else are
     *  dropped (audit A7). */
    @Volatile private var daemonAddr: InetSocketAddress? = null

    /** Whether the link is up, and if not why (audit A7). */
    @Volatile private var tracker = LinkTracker(0, NO_REPLY_AFTER_MS, TIMEOUT_MS)

    /** Keep the CPU and Wi-Fi awake with the screen off (audit A8). */
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    /** Rebinds the socket as networks come and go (audit A8). */
    private var networkWatch: ConnectivityManager.NetworkCallback? = null
    private var boundNetwork: Network? = null

    /** Input names arrive on their own messages, possibly before the sends —
     *  cache them so sends pick up the right label whenever they appear.
     *  Written on the link's thread, read on the UI thread too (audit A12). */
    private val inputNames = ConcurrentHashMap<Int, String>()

    inner class LocalBinder : Binder() {
        fun service(): MonitorService = this@MonitorService
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!goForeground()) {
            // Android won't let the service into the foreground right now,
            // e.g. on a sticky restart while the app is in the background.
            // Stop instead of crashing; opening the app starts it again.
            stopSelf()
            return START_NOT_STICKY
        }
        val c = CredentialsStore.load(this)
        if (c == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        creds = c
        startNetworking(c)
        return START_STICKY
    }

    /**
     * Enters the foreground as a `connectedDevice` service, which, unlike
     * `dataSync`, has no daily time limit on Android 15+ (audit A4). Returns
     * false if Android refuses: `ForegroundServiceStartNotAllowedException`
     * when started from the background, or a `SecurityException`.
     */
    private fun goForeground(): Boolean {
        val notification = buildNotification()
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
                )
            } else {
                @Suppress("DEPRECATION")
                startForeground(NOTIFICATION_ID, notification)
            }
            true
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't start in the foreground", e)
            false
        }
    }

    /**
     * Android 15+ calls this when a foreground service type with a time limit
     * runs out. `connectedDevice` has none, so this is a safety net: end the
     * link and stop, rather than be killed for overrunning (audit A4), and
     * leave a notification saying so.
     */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stopLink()
        // Say so in the app too, rather than a bare "Connecting…".
        _state.update { it.copy(problem = LinkProblem.STOPPED) }
        stopForeground(STOP_FOREGROUND_DETACH)
        getSystemService(NotificationManager::class.java).notify(
            NOTIFICATION_ID,
            NotificationCompat.Builder(this, CHANNEL_ID)
                .setContentTitle(getString(R.string.app_name))
                .setContentText("Android stopped the monitor link. Open the app to reconnect.")
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setContentIntent(openAppIntent())
                .setAutoCancel(true)
                .build(),
        )
        stopSelf()
    }

    private fun startNetworking(c: Credentials) {
        if (running) return
        running = true

        val sock = DatagramSocket() // ephemeral local port; daemon replies to it
        sock.broadcast = true
        sock.soTimeout = 1000
        socket = sock

        val linkJob = SupervisorJob(job)
        link = linkJob
        val scope = CoroutineScope(Dispatchers.IO + linkJob)

        daemonAddr = null
        tracker = LinkTracker(SystemClock.elapsedRealtime(), NO_REPLY_AFTER_MS, TIMEOUT_MS)
        holdAwake()
        watchNetworks(sock)

        val queue = LinkedBlockingQueue<ByteArray>(1024)

        // Receive thread: blocking reads, copy + enqueue, drop oldest if full.
        receiveThread = Thread({
            val buf = ByteArray(8192)
            while (running && !sock.isClosed) {
                val pkt = DatagramPacket(buf, buf.size)
                try {
                    sock.receive(pkt)
                    // Only the daemon's packets count (audit A7). A monitor
                    // reply from elsewhere is still noted: a daemon with
                    // several addresses may answer from another one, and
                    // "no reply" would then mislead.
                    if (pkt.address != daemonAddr?.address) {
                        if (daemonAddr != null && isMonitorReply(pkt)) noteStrayReply(pkt.address)
                        continue
                    }
                    val data = pkt.data.copyOf(pkt.length)
                    if (!queue.offer(data)) {
                        queue.poll()
                        queue.offer(data)
                    }
                } catch (e: SocketTimeoutException) {
                    // expected — lets the loop re-check `running`
                } catch (e: Exception) {
                    if (!running) break
                }
            }
        }, "monitor-rx").also { it.start() }

        // Processing loop: drain queue, decode, fold into state.
        scope.launch {
            while (isActive && running) {
                val data = queue.poll(200, TimeUnit.MILLISECONDS) ?: continue
                OscCodec.decode(data)?.let { onInbound(it) }
            }
        }

        // Sender: the only place packets leave, so they go out in the order
        // they were queued (audit A3). The daemon's address is resolved once
        // per link, not per packet.
        scope.launch {
            while (isActive) {
                when (val wait = outbound.msUntilNextResend(SystemClock.elapsedRealtime())) {
                    null -> wakeSender.receive()
                    0L -> {}
                    else -> withTimeoutOrNull(wait) { wakeSender.receive() }
                }
                val packets = outbound.take(SystemClock.elapsedRealtime())
                if (packets.isEmpty()) continue
                // Unresolvable for now: drop this batch; the heartbeat retries.
                val to = daemonAddr ?: resolve(c)?.also {
                    daemonAddr = it
                    bindToNetwork(sock)
                } ?: continue
                for (bytes in packets) {
                    try {
                        sock.send(DatagramPacket(bytes, bytes.size, to))
                    } catch (e: Exception) {
                        // transient network error — the resend and heartbeat recover
                    }
                }
            }
        }

        // Initial connect + state request.
        send(MonitorProtocol.connect(c.name))
        send(MonitorProtocol.requestState(c.name))

        // Heartbeat every 10 s: the daemon's keepalive for this profile, and
        // its reply is the profile's full state. Also renews the wake and
        // Wi-Fi locks, but only while the link is up or recently was: after
        // that the phone may sleep until a reply brings the link back
        // (audit R5).
        scope.launch {
            while (isActive && running) {
                send(MonitorProtocol.connect(c.name))
                if (tracker.keepAwake(SystemClock.elapsedRealtime(), AWAKE_GRACE_MS)) {
                    renewAwake()
                } else {
                    releaseAwake()
                }
                delay(HEARTBEAT_MS)
            }
        }

        // Ping every 2 s: a one-packet reply, so a dropped link shows within
        // seconds rather than after the next heartbeat (audit A7). Slower
        // once the link has been down for a while (audit R5).
        scope.launch {
            while (isActive && running) {
                val awake = tracker.keepAwake(SystemClock.elapsedRealtime(), AWAKE_GRACE_MS)
                delay(if (awake) PING_MS else PING_IDLE_MS)
                send(MonitorProtocol.PING)
            }
        }

        // Watchdog: turns silence into NO_REPLY or LOST.
        scope.launch {
            while (isActive && running) {
                delay(WATCHDOG_MS)
                refreshLink(SystemClock.elapsedRealtime())
            }
        }
    }

    /** Publishes the tracker's view of the link when it changes (audit A7).
     *  Called from the inbound loop and the watchdog: synchronized, or a
     *  stale LOST could overwrite a fresh connected state. */
    @Synchronized
    private fun refreshLink(nowMs: Long) {
        val status = tracker.status(nowMs)
        val st = _state.value
        if (st.connected == status.connected && st.problem == status.problem) return
        // Back after a drop: anything changed meanwhile needs a fresh snapshot.
        if (status.connected && st.problem == LinkProblem.LOST) {
            creds?.let { send(MonitorProtocol.requestState(it.name)) }
        }
        // Back after the locks were let go: take them again now, not at the
        // next heartbeat (audit R5).
        if (status.connected && running) renewAwake()
        _state.update { it.copy(connected = status.connected, problem = status.problem) }
        updateNotification()
    }

    private fun isMonitorReply(pkt: DatagramPacket): Boolean =
        OscCodec.decode(pkt.data, pkt.length)?.let { MonitorProtocol.parse(it) } != null

    private fun noteStrayReply(from: InetAddress) {
        tracker.strayReplyReceived()
        val addr = from.hostAddress ?: return
        if (_state.value.replyFrom != addr) _state.update { it.copy(replyFrom = addr) }
    }

    /** Keeps the CPU awake so the heartbeat runs with the screen off (the
     *  daemon drops a client after 30 s of silence), and Wi-Fi out of power
     *  save (audit A8). The wake lock times out unless the heartbeat renews it,
     *  and the heartbeat lets both go once the link has been down for
     *  [AWAKE_GRACE_MS] (audit R5). */
    @Synchronized
    private fun holdAwake() {
        wakeLock = getSystemService(PowerManager::class.java)
            ?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "S21Monitor:link")
            ?.apply {
                setReferenceCounted(false)
                acquire(WAKE_LOCK_MS)
            }
        // HIGH_PERF works with the screen off but does nothing from Android 14;
        // LOW_LATENCY then helps at least while the screen is on.
        val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            WifiManager.WIFI_MODE_FULL_LOW_LATENCY
        } else {
            @Suppress("DEPRECATION")
            WifiManager.WIFI_MODE_FULL_HIGH_PERF
        }
        wifiLock = applicationContext.getSystemService(WifiManager::class.java)
            ?.createWifiLock(mode, "S21Monitor:link")
            ?.apply {
                setReferenceCounted(false)
                acquire()
            }
    }

    /** Extends the locks, or takes them again after [releaseAwake]. */
    @Synchronized
    private fun renewAwake() {
        val wake = wakeLock
        val wifi = wifiLock
        if (wake == null || wifi == null) {
            releaseAwake()
            holdAwake()
            return
        }
        wake.acquire(WAKE_LOCK_MS)
        if (!wifi.isHeld) wifi.acquire()
    }

    @Synchronized
    private fun releaseAwake() {
        wakeLock?.takeIf { it.isHeld }?.release()
        wakeLock = null
        wifiLock?.takeIf { it.isHeld }?.release()
        wifiLock = null
    }

    /** Rebinds the socket whenever networks come, go or change (audit A8). */
    private fun watchNetworks(sock: DatagramSocket) {
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = bindToNetwork(sock)
            override fun onLost(network: Network) = bindToNetwork(sock)
            override fun onLinkPropertiesChanged(network: Network, lp: LinkProperties) =
                bindToNetwork(sock)
        }
        // Not only networks with internet: show Wi-Fi often has none.
        val request = NetworkRequest.Builder()
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        try {
            cm.registerNetworkCallback(request, callback)
            networkWatch = callback
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't watch networks", e)
        }
    }

    /**
     * Binds the socket to the network that reaches the daemon (see
     * [chooseNetwork]): on show Wi-Fi without internet, Android would
     * otherwise send it over mobile data (audit A8).
     */
    @Synchronized
    private fun bindToNetwork(sock: DatagramSocket) {
        val daemon = daemonAddr?.address ?: return
        if (sock.isClosed) return
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val pick = chooseNetwork(networkOptions(cm, daemon)) ?: return
        if (pick == boundNetwork) return
        try {
            pick.bindSocket(sock)
            boundNetwork = pick
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't bind the link to $pick", e)
        }
    }

    private fun onInbound(msg: com.pob31.s21monitor.osc.OscMessage) {
        val now = SystemClock.elapsedRealtime()
        val ev = MonitorProtocol.parse(msg)
        tracker.received(
            when (ev) {
                is Inbound.SendFull, is Inbound.SendEcho, is Inbound.AuxStrip,
                is Inbound.NameInput, is Inbound.NameAux -> Reply.PROFILE
                is Inbound.Error ->
                    if (ev.kind == "unknown_client") Reply.UNKNOWN_NAME else Reply.PONG
                else -> Reply.PONG
            },
            now,
        )
        refreshLink(now)

        // A control the musician is using keeps its own value (audit A5).
        when (ev) {
            is Inbound.SendFull -> updateSend(ev.input, ev.aux) { s ->
                s.copy(
                    level = holds.pick(Control.SendLevel(ev.input, ev.aux), now, s.level, ev.level),
                    pan = holds.pick(Control.SendPan(ev.input, ev.aux), now, s.pan, ev.pan),
                    on = holds.pick(Control.SendOn(ev.input, ev.aux), now, s.on, ev.on),
                )
            }

            is Inbound.SendEcho -> _state.update { st ->
                st.withSendEcho(ev.input, ev.aux) { s ->
                    when (ev.param) {
                        "level" -> s.copy(level = holds.pick(
                            Control.SendLevel(ev.input, ev.aux), now, s.level, MonitorProtocol.asFloat(ev.arg)))
                        "pan" -> s.copy(pan = holds.pick(
                            Control.SendPan(ev.input, ev.aux), now, s.pan, MonitorProtocol.asFloat(ev.arg)))
                        "on" -> s.copy(on = holds.pick(
                            Control.SendOn(ev.input, ev.aux), now, s.on, MonitorProtocol.asBool(ev.arg)))
                        else -> s
                    }
                }
            }

            is Inbound.AuxStrip -> updateAux(ev.aux) { a ->
                a.copy(
                    fader = holds.pick(Control.AuxFader(ev.aux), now, a.fader, ev.fader),
                    mute = holds.pick(Control.AuxMute(ev.aux), now, a.mute, ev.mute),
                )
            }

            is Inbound.NameInput -> {
                inputNames[ev.input] = ev.name
                _state.update { st ->
                    st.copy(sends = st.sends.mapValues { (k, v) ->
                        if (k.first == ev.input) v.copy(name = ev.name) else v
                    })
                }
            }

            is Inbound.NameAux -> updateAux(ev.aux) { it.copy(name = ev.name) }

            is Inbound.Discovered -> _state.update { it.copy(console = ev.console) }

            is Inbound.Error, Inbound.Pong, null -> {}
        }
    }

    private fun updateSend(input: Int, aux: Int, transform: (SendState) -> SendState) {
        _state.update { st ->
            val key = input to aux
            val base = st.sends[key] ?: SendState(input, aux, name = inputNames[input] ?: "")
            st.sends.toMutableMap().also { it[key] = transform(base) }.let { st.copy(sends = it) }
        }
    }

    private fun updateAux(aux: Int, transform: (AuxState) -> AuxState) {
        _state.update { st ->
            val base = st.auxes[aux] ?: AuxState(aux)
            st.auxes.toMutableMap().also { it[aux] = transform(base) }.let { st.copy(auxes = it) }
        }
    }

    // ── Commands from the UI (via the binder) — optimistic local update + send ──

    /** A finger went down on, or lifted off, [control] (audit A5). */
    fun touch(control: Control, down: Boolean) =
        holds.touch(control, down, SystemClock.elapsedRealtime())

    fun setSendLevel(input: Int, aux: Int, value: Float) {
        holds.changed(Control.SendLevel(input, aux), SystemClock.elapsedRealtime())
        updateSend(input, aux) { it.copy(level = value) }
        creds?.let { send(MonitorProtocol.sendLevel(it.name, input, aux), OscFloat(value), resend = true) }
    }

    fun setSendPan(input: Int, aux: Int, value: Float) {
        holds.changed(Control.SendPan(input, aux), SystemClock.elapsedRealtime())
        updateSend(input, aux) { it.copy(pan = value) }
        creds?.let { send(MonitorProtocol.sendPan(it.name, input, aux), OscFloat(value), resend = true) }
    }

    fun setSendOn(input: Int, aux: Int, on: Boolean) {
        holds.changed(Control.SendOn(input, aux), SystemClock.elapsedRealtime())
        updateSend(input, aux) { it.copy(on = on) }
        creds?.let { send(MonitorProtocol.sendOn(it.name, input, aux), OscBool(on), resend = true) }
    }

    fun setAuxFader(aux: Int, value: Float) {
        holds.changed(Control.AuxFader(aux), SystemClock.elapsedRealtime())
        updateAux(aux) { it.copy(fader = value) }
        creds?.let { send(MonitorProtocol.auxFader(it.name, aux), OscFloat(value), resend = true) }
    }

    fun setAuxMute(aux: Int, mute: Boolean) {
        holds.changed(Control.AuxMute(aux), SystemClock.elapsedRealtime())
        updateAux(aux) { it.copy(mute = mute) }
        creds?.let { send(MonitorProtocol.auxMute(it.name, aux), OscBool(mute), resend = true) }
    }

    /** Stop the link and the service entirely (UI's "shut down" action). */
    fun shutdown() {
        stopLink()
        stopSelf()
    }

    /**
     * Queues a packet for the link's sender. [resend] marks a control value:
     * it is sent once more after it stops changing (see [OutboundQueue]).
     */
    private fun send(address: String, vararg args: OscArg, resend: Boolean = false) {
        if (!running) return
        val bytes = OscCodec.encode(address, args.toList())
        outbound.offer(address, bytes, SystemClock.elapsedRealtime(), resend)
        wakeSender.trySend(Unit)
    }

    private fun resolve(c: Credentials): InetSocketAddress? =
        runCatching { InetSocketAddress(InetAddress.getByName(c.host), c.port) }.getOrNull()

    /** Ends the current link: its coroutines, receive thread, socket,
     *  network watch and locks. */
    private fun stopLink() {
        running = false
        link?.cancel()
        link = null
        receiveThread?.interrupt()
        receiveThread = null
        runCatching { socket?.close() }
        socket = null
        networkWatch?.let { cb ->
            runCatching { getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(cb) }
        }
        networkWatch = null
        boundNetwork = null
        daemonAddr = null
        releaseAwake()
        outbound.clear()
        _state.update { it.copy(connected = false, problem = null, replyFrom = null) }
    }

    override fun onDestroy() {
        stopLink()
        job.cancel()
        super.onDestroy()
    }

    // ── Notification ──

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                getString(R.string.notif_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = getString(R.string.notif_channel_desc)
                setShowBadge(false)
                enableVibration(false)
            }
            getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }
    }

    private fun updateNotification() {
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, buildNotification())
    }

    private fun openAppIntent(): PendingIntent {
        val tapIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getActivity(this, 0, tapIntent, flags)
    }

    private fun buildNotification(): Notification {
        val pending = openAppIntent()

        val s = _state.value
        val label = s.console.ifEmpty { "console" }
        val name = creds?.name.orEmpty()
        val text = when {
            s.connected -> "Connected to $label as $name"
            s.problem == LinkProblem.UNKNOWN_NAME -> "The daemon doesn't know the name \u201c$name\u201d"
            s.problem == LinkProblem.NO_REPLY -> "No reply from the daemon"
            s.problem == LinkProblem.LOST -> "Connection lost, retrying…"
            s.problem == LinkProblem.OTHER_ADDRESS -> "The daemon answers from ${s.replyFrom}"
            s.problem == LinkProblem.STOPPED -> "Android stopped the monitor link"
            else -> "Connecting to $label…"
        }

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentIntent(pending)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .build()
    }

    companion object {
        private const val TAG = "MonitorService"
        /** How long a control must be still before its value is resent. */
        private const val RESEND_AFTER_MS = 250L
        /** How long a control keeps ignoring incoming values after release. */
        private const val HOLD_AFTER_RELEASE_MS = 300L
        private const val NOTIFICATION_ID = 8521
        private const val CHANNEL_ID = "s21_monitor_service"
        private const val HEARTBEAT_MS = 10_000L
        private const val PING_MS = 2_000L
        private const val WATCHDOG_MS = 1_000L
        /** No reply for this long after starting: [LinkProblem.NO_REPLY]. */
        private const val NO_REPLY_AFTER_MS = 5_000L
        /** Silent for this long (three missed pings): [LinkProblem.LOST]. */
        private const val TIMEOUT_MS = 6_000L
        /** The wake lock's timeout; the heartbeat renews it well before. */
        private const val WAKE_LOCK_MS = 60_000L
        /** How long after the link was last up the phone is still kept awake
         *  for it; then the locks go and pings slow down (audit R5). */
        private const val AWAKE_GRACE_MS = 5 * 60_000L
        /** Ping interval once the link has been down past [AWAKE_GRACE_MS]. */
        private const val PING_IDLE_MS = 15_000L
    }
}

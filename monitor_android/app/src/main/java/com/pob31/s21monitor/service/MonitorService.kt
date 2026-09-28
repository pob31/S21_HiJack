package com.pob31.s21monitor.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import com.pob31.s21monitor.R
import com.pob31.s21monitor.data.CredentialsStore
import com.pob31.s21monitor.model.AuxState
import com.pob31.s21monitor.model.Credentials
import com.pob31.s21monitor.model.MonitorUiState
import com.pob31.s21monitor.model.SendState
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

    @Volatile private var running = false
    @Volatile private var lastRxMs = 0L

    /** Input names arrive on their own messages, possibly before the sends —
     *  cache them so sends pick up the right label whenever they appear. */
    private val inputNames = HashMap<Int, String>()

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

        val queue = LinkedBlockingQueue<ByteArray>(1024)

        // Receive thread: blocking reads, copy + enqueue, drop oldest if full.
        receiveThread = Thread({
            val buf = ByteArray(8192)
            while (running && !sock.isClosed) {
                val pkt = DatagramPacket(buf, buf.size)
                try {
                    sock.receive(pkt)
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
            var daemon: InetSocketAddress? = null
            while (isActive) {
                when (val wait = outbound.msUntilNextResend(SystemClock.elapsedRealtime())) {
                    null -> wakeSender.receive()
                    0L -> {}
                    else -> withTimeoutOrNull(wait) { wakeSender.receive() }
                }
                val packets = outbound.take(SystemClock.elapsedRealtime())
                if (packets.isEmpty()) continue
                // Unresolvable for now: drop this batch; the heartbeat retries.
                val to = daemon ?: resolve(c)?.also { daemon = it } ?: continue
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

        // Heartbeat every 10 s (also the connect/keepalive).
        scope.launch {
            while (isActive && running) {
                send(MonitorProtocol.connect(c.name))
                delay(HEARTBEAT_MS)
            }
        }

        // Watchdog: after 15 s of silence, mark disconnected and re-handshake.
        scope.launch {
            while (isActive && running) {
                delay(WATCHDOG_MS)
                val silentFor = System.currentTimeMillis() - lastRxMs
                if (lastRxMs != 0L && silentFor > TIMEOUT_MS) {
                    if (_state.value.connected) _state.update { it.copy(connected = false) }
                    send(MonitorProtocol.connect(c.name))
                    send(MonitorProtocol.requestState(c.name))
                }
                updateNotification()
            }
        }
    }

    private fun onInbound(msg: com.pob31.s21monitor.osc.OscMessage) {
        lastRxMs = System.currentTimeMillis()
        if (!_state.value.connected) _state.update { it.copy(connected = true) }

        when (val ev = MonitorProtocol.parse(msg)) {
            is Inbound.SendFull ->
                updateSend(ev.input, ev.aux) { it.copy(level = ev.level, pan = ev.pan, on = ev.on) }

            is Inbound.SendEcho -> _state.update { st ->
                st.withSendEcho(ev.input, ev.aux) { s ->
                    when (ev.param) {
                        "level" -> s.copy(level = MonitorProtocol.asFloat(ev.arg))
                        "pan" -> s.copy(pan = MonitorProtocol.asFloat(ev.arg))
                        "on" -> s.copy(on = MonitorProtocol.asBool(ev.arg))
                        else -> s
                    }
                }
            }

            is Inbound.AuxStrip ->
                updateAux(ev.aux) { it.copy(fader = ev.fader, mute = ev.mute) }

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

            null -> { /* unrecognised */ }
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

    fun setSendLevel(input: Int, aux: Int, value: Float) {
        updateSend(input, aux) { it.copy(level = value) }
        creds?.let { send(MonitorProtocol.sendLevel(it.name, input, aux), OscFloat(value), resend = true) }
    }

    fun setSendPan(input: Int, aux: Int, value: Float) {
        updateSend(input, aux) { it.copy(pan = value) }
        creds?.let { send(MonitorProtocol.sendPan(it.name, input, aux), OscFloat(value), resend = true) }
    }

    fun setSendOn(input: Int, aux: Int, on: Boolean) {
        updateSend(input, aux) { it.copy(on = on) }
        creds?.let { send(MonitorProtocol.sendOn(it.name, input, aux), OscBool(on), resend = true) }
    }

    fun setAuxFader(aux: Int, value: Float) {
        updateAux(aux) { it.copy(fader = value) }
        creds?.let { send(MonitorProtocol.auxFader(it.name, aux), OscFloat(value), resend = true) }
    }

    fun setAuxMute(aux: Int, mute: Boolean) {
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

    /** Ends the current link: its coroutines, receive thread and socket. */
    private fun stopLink() {
        running = false
        link?.cancel()
        link = null
        receiveThread?.interrupt()
        receiveThread = null
        runCatching { socket?.close() }
        socket = null
        outbound.clear()
        if (_state.value.connected) _state.update { it.copy(connected = false) }
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
        val text = if (s.connected) {
            "Connected to $label as ${creds?.name ?: ""}"
        } else {
            "Connecting to $label…"
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
        private const val NOTIFICATION_ID = 8521
        private const val CHANNEL_ID = "s21_monitor_service"
        private const val HEARTBEAT_MS = 10_000L
        private const val WATCHDOG_MS = 5_000L
        private const val TIMEOUT_MS = 15_000L
    }
}

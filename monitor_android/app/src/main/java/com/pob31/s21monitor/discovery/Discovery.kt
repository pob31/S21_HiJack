package com.pob31.s21monitor.discovery

import com.pob31.s21monitor.osc.MonitorProtocol
import com.pob31.s21monitor.osc.OscCodec
import com.pob31.s21monitor.osc.OscString
import com.pob31.s21monitor.osc.OscInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException

/** Finds S21_HiJack daemons on the LAN via a UDP broadcast probe. */
object Discovery {

    data class Found(val host: String, val port: Int, val console: String) {
        override fun toString() = "$console ($host:$port)"
    }

    /**
     * Broadcast `/monitor/discover` and collect `/monitor/discovered` replies
     * for [timeoutMs]. Unlike the Flutter client, this captures the daemon's
     * host from the reply's *source* address and uses the port the daemon
     * reports, so the result is directly usable to connect.
     */
    suspend fun discover(port: Int = 8025, timeoutMs: Long = 3000): List<Found> =
        withContext(Dispatchers.IO) {
            val found = LinkedHashMap<String, Found>()
            runCatching {
                DatagramSocket().use { sock ->
                    sock.broadcast = true
                    sock.soTimeout = 300
                    val probe = OscCodec.encode(MonitorProtocol.DISCOVER, emptyList())
                    sock.send(
                        DatagramPacket(
                            probe, probe.size,
                            InetAddress.getByName("255.255.255.255"), port,
                        )
                    )
                    val deadline = System.currentTimeMillis() + timeoutMs
                    val buf = ByteArray(2048)
                    while (System.currentTimeMillis() < deadline) {
                        val pkt = DatagramPacket(buf, buf.size)
                        try {
                            sock.receive(pkt)
                        } catch (e: SocketTimeoutException) {
                            continue
                        }
                        val msg = OscCodec.decode(pkt.data, pkt.length) ?: continue
                        if (msg.address != "/monitor/discovered") continue
                        val host = pkt.address?.hostAddress ?: continue
                        val console = (msg.args.getOrNull(0) as? OscString)?.value ?: ""
                        val replyPort = (msg.args.getOrNull(1) as? OscInt)?.value?.takeIf { it > 0 } ?: port
                        found[host] = Found(host, replyPort, console)
                    }
                }
            }
            found.values.toList()
        }
}

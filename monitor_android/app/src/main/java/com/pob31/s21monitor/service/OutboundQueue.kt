package com.pob31.s21monitor.service

/**
 * Outbound OSC packets waiting for [MonitorService]'s single sender, which
 * sends them in order (audit A3).
 *
 * - One packet per OSC address: a newer packet replaces one still waiting,
 *   so a fast fader drag sends its latest value rather than a backlog.
 * - A value packet (offered with `resend = true`) is sent a second time once
 *   its address has been quiet for [resendAfterMs], so losing the last UDP
 *   packet of a drag or a tap doesn't leave the desk on an older value.
 *
 * Thread-safe. Times are passed in (ms, any monotonic clock) so tests can
 * drive them.
 */
class OutboundQueue(private val resendAfterMs: Long) {

    private val waiting = LinkedHashMap<String, ByteArray>()
    private val resends = LinkedHashMap<String, Resend>()

    private class Resend(val dueMs: Long, val packet: ByteArray)

    @Synchronized
    fun offer(address: String, packet: ByteArray, nowMs: Long, resend: Boolean) {
        waiting[address] = packet
        if (resend) resends[address] = Resend(nowMs + resendAfterMs, packet)
    }

    /** The packets to send now: everything waiting, oldest address first,
     *  then any resends that are due. */
    @Synchronized
    fun take(nowMs: Long): List<ByteArray> {
        val out = ArrayList<ByteArray>(waiting.values)
        waiting.clear()
        val due = resends.entries.filter { it.value.dueMs <= nowMs }
        for (e in due) {
            out += e.value.packet
            resends.remove(e.key)
        }
        return out
    }

    /** Ms until the next resend is due (0 if one is due now), or null if
     *  none is scheduled. */
    @Synchronized
    fun msUntilNextResend(nowMs: Long): Long? =
        resends.values.minOfOrNull { it.dueMs }?.let { (it - nowMs).coerceAtLeast(0) }

    @Synchronized
    fun clear() {
        waiting.clear()
        resends.clear()
    }
}

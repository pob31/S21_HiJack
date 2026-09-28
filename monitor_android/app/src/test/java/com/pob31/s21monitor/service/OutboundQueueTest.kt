package com.pob31.s21monitor.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Audit A3: sends go out in order, conflated per address, and the final
 *  value of a control is sent twice. */
class OutboundQueueTest {

    private fun p(s: String) = s.toByteArray()
    private fun List<ByteArray>.strings() = map { String(it) }

    @Test
    fun aNewerValueReplacesTheOneStillWaiting() {
        val q = OutboundQueue(resendAfterMs = 250)
        q.offer("/level", p("-80"), 0, resend = false)
        q.offer("/level", p("-60"), 1, resend = false)

        assertEquals(listOf("-60"), q.take(2).strings())
        assertEquals(emptyList<String>(), q.take(3).strings())
    }

    @Test
    fun addressesGoOutInTheOrderTheyWereFirstQueued() {
        val q = OutboundQueue(resendAfterMs = 250)
        q.offer("/connect", p("connect"), 0, resend = false)
        q.offer("/state", p("state"), 0, resend = false)
        q.offer("/connect", p("connect again"), 0, resend = false)

        assertEquals(listOf("connect again", "state"), q.take(0).strings())
    }

    @Test
    fun theLastValueIsSentAgainOnceTheControlGoesQuiet() {
        val q = OutboundQueue(resendAfterMs = 250)
        q.offer("/level", p("-80"), 0, resend = true)
        assertEquals(listOf("-80"), q.take(0).strings())

        // Still moving: every new value pushes the resend back.
        q.offer("/level", p("-60"), 200, resend = true)
        assertEquals(listOf("-60"), q.take(200).strings())
        assertEquals(250L, q.msUntilNextResend(200))
        assertEquals(emptyList<String>(), q.take(300).strings())

        // Quiet for 250 ms: the final value goes out once more, and only once.
        assertEquals(listOf("-60"), q.take(450).strings())
        assertNull(q.msUntilNextResend(450))
        assertEquals(emptyList<String>(), q.take(1000).strings())
    }

    @Test
    fun keepAlivesAreNotResent() {
        val q = OutboundQueue(resendAfterMs = 250)
        q.offer("/connect", p("connect"), 0, resend = false)
        q.take(0)

        assertNull(q.msUntilNextResend(0))
        assertEquals(emptyList<String>(), q.take(1000).strings())
    }

    @Test
    fun clearDropsEverything() {
        val q = OutboundQueue(resendAfterMs = 250)
        q.offer("/level", p("-60"), 0, resend = true)
        q.clear()

        assertNull(q.msUntilNextResend(0))
        assertEquals(emptyList<String>(), q.take(1000).strings())
    }
}

package com.pob31.s21monitor.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

class MonitorStateTest {

    private fun stateWith(vararg sends: SendState) =
        MonitorUiState(sends = sends.associateBy { it.input to it.aux })

    /** Audit A1: an echo for a send this client was never given must not add
     *  a strip, or every later strip in the row would shift by one. */
    @Test
    fun echoForAnUnknownSendIsIgnored() {
        val st = stateWith(SendState(3, 1, level = -10f), SendState(7, 1, level = -20f))

        val after = st.withSendEcho(5, 1) { it.copy(level = 0f) }

        assertSame(st, after)
        assertEquals(listOf(3, 7), after.sendsForAux(1).map { it.input })
    }

    @Test
    fun echoForAKnownSendIsApplied() {
        val st = stateWith(SendState(3, 1, level = -10f), SendState(3, 2, level = -10f))

        val after = st.withSendEcho(3, 2) { it.copy(level = -4f) }

        assertEquals(-4f, after.sends[3 to 2]!!.level)
        assertEquals(-10f, after.sends[3 to 1]!!.level)
    }
}

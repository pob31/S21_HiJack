package com.pob31.s21monitor.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Audit A5: incoming values don't move a control under the finger. */
class ControlHoldsTest {

    private val level = Control.SendLevel(3, 1)

    @Test
    fun heldWhileTouchedHoweverLong() {
        val h = ControlHolds(holdMs = 300)
        h.touch(level, down = true, nowMs = 0)

        assertTrue(h.isHeld(level, 60_000))
        assertEquals(-20f, h.pick(level, 60_000, local = -20f, incoming = -35f))
    }

    @Test
    fun heldBrieflyAfterRelease() {
        val h = ControlHolds(holdMs = 300)
        h.touch(level, down = true, nowMs = 0)
        h.touch(level, down = false, nowMs = 1_000)

        assertTrue(h.isHeld(level, 1_299))
        assertFalse(h.isHeld(level, 1_300))
        assertEquals(-35f, h.pick(level, 1_300, local = -20f, incoming = -35f))
    }

    @Test
    fun aChangeMadeHereHoldsWithoutATouch() {
        // Taps (ON, MUTE, double-tap to centre) never report a touch.
        val h = ControlHolds(holdMs = 300)
        val on = Control.SendOn(3, 1)
        h.changed(on, nowMs = 0)

        assertTrue(h.isHeld(on, 299))
        assertFalse(h.isHeld(on, 300))
    }

    @Test
    fun otherControlsAreNotHeld() {
        val h = ControlHolds(holdMs = 300)
        h.touch(level, down = true, nowMs = 0)

        assertFalse(h.isHeld(Control.SendPan(3, 1), 0))
        assertFalse(h.isHeld(Control.SendLevel(3, 2), 0))
        assertFalse(h.isHeld(Control.AuxFader(1), 0))
    }
}

package com.pob31.s21monitor.ui.widgets

import com.pob31.s21monitor.model.OFF_DB
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Audit A2 (relative drag) and A6 (fader law, "-inf" only when off). */
class FaderDragTest {

    private val height = 1000f // px; 10 px = 1% of travel

    @Test
    fun noTravelKeepsTheStartingLevel() {
        // The old fader jumped to wherever the finger landed.
        assertEquals(-30f, dragDb(-30f, 0f, height), 1e-3f)
    }

    @Test
    fun travelMovesAlongTheFaderLaw() {
        // Unity sits at 3/4 travel; −10 dB at 60%, +10 dB at the top.
        assertEquals(FADER_MAX_DB, dragDb(0f, -250f, height), 1e-3f)
        assertEquals(-10f, dragDb(0f, 150f, height), 1e-3f)
    }

    @Test
    fun theLevelStaysOnTheFader() {
        assertEquals(FADER_MAX_DB, dragDb(0f, -5000f, height), 0f)
        assertEquals(OFF_DB, dragDb(-30f, 5000f, height), 0f)
    }

    @Test
    fun theBottomIsOffAndOnlyTheBottom() {
        assertEquals(OFF_DB, dragDb(-60f, 100f, height), 0f) // −60 dB sits at 10%
        assertEquals(OFF_DB, dragDb(OFF_DB, 0f, height), 0f)
        assertEquals(-90f, dragDb(OFF_DB, -20f, height), 1e-3f) // up 2%
    }

    @Test
    fun anUnusableStartCountsAsOff() {
        assertEquals(OFF_DB, dragDb(Float.NaN, 0f, height), 0f)
        assertEquals(OFF_DB, dragDb(Float.POSITIVE_INFINITY, 0f, height), 0f)
    }

    @Test
    fun theLawRoundTrips() {
        var f = 0.01f
        while (f <= 1f) {
            assertEquals(f, dbToFraction(fractionToDb(f)), 1e-4f)
            f += 0.01f
        }
        assertEquals(0.75f, dbToFraction(0f), 1e-6f)
    }

    @Test
    fun minusInfIsShownOnlyWhenOff() {
        // The old label read "-inf" from −59 dB down, 23% of the travel.
        assertEquals("-inf", formatDb(OFF_DB))
        assertNotEquals("-inf", formatDb(-60f))
        assertTrue(formatDb(-89f).contains("89"))
    }
}

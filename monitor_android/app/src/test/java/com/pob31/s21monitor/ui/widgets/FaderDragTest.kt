package com.pob31.s21monitor.ui.widgets

import com.pob31.s21monitor.model.OFF_DB
import org.junit.Assert.assertEquals
import org.junit.Test

/** Audit A2: a fader drag is relative to the level it started from. */
class FaderDragTest {

    private val height = 900f // px; the full height spans 90 dB, so 10 px = 1 dB

    @Test
    fun noTravelKeepsTheStartingLevel() {
        // The old fader jumped to wherever the finger landed.
        assertEquals(-30f, dragDb(-30f, 0f, height), 1e-4f)
    }

    @Test
    fun travelMovesTheLevelByTheSameShareOfTheRange() {
        assertEquals(-24f, dragDb(-30f, -60f, height), 1e-4f) // up 60 px
        assertEquals(-40f, dragDb(-30f, 100f, height), 1e-4f) // down 100 px
    }

    @Test
    fun theLevelStaysOnTheFader() {
        assertEquals(FADER_MAX_DB, dragDb(0f, -5000f, height), 0f)
        assertEquals(FADER_MIN_DB, dragDb(-30f, 5000f, height), 0f)
    }

    @Test
    fun offCountsAsTheBottomOfTheFader() {
        assertEquals(FADER_MIN_DB, dragDb(OFF_DB, 0f, height), 0f)
        assertEquals(FADER_MIN_DB + 5f, dragDb(OFF_DB, -50f, height), 1e-4f)
    }

    @Test
    fun anUnusableStartCountsAsTheBottom() {
        assertEquals(FADER_MIN_DB, dragDb(Float.NaN, 0f, height), 0f)
        assertEquals(FADER_MIN_DB, dragDb(Float.POSITIVE_INFINITY, 0f, height), 0f)
    }
}

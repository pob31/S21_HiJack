package com.pob31.s21monitor.service

import com.pob31.s21monitor.model.LinkProblem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Audit A7: the link state behind the lock on the controls. */
class LinkTrackerTest {

    private fun tracker() = LinkTracker(startMs = 0, noReplyAfterMs = 5_000, timeoutMs = 6_000)
    private val connecting = LinkStatus(false, null)
    private val up = LinkStatus(true, null)

    @Test
    fun silenceFromTheStartBecomesNoReply() {
        val t = tracker()
        assertEquals(connecting, t.status(4_999))
        assertEquals(LinkStatus(false, LinkProblem.NO_REPLY), t.status(5_000))
    }

    @Test
    fun profileStateMeansUp() {
        val t = tracker()
        t.received(Reply.PROFILE, 100)
        assertEquals(up, t.status(200))
    }

    @Test
    fun aPingReplyAloneIsNotEnough() {
        // The daemon answers pings from anyone, known name or not.
        val t = tracker()
        t.received(Reply.PONG, 100)
        assertEquals(connecting, t.status(3_000))
    }

    @Test
    fun anUnknownNameIsReportedAndPingsDoNotHideIt() {
        val t = tracker()
        t.received(Reply.UNKNOWN_NAME, 100)
        t.received(Reply.PONG, 2_000)
        assertEquals(LinkStatus(false, LinkProblem.UNKNOWN_NAME), t.status(2_100))

        // The engineer adds the profile: the next heartbeat brings its state.
        t.received(Reply.PROFILE, 10_000)
        assertEquals(up, t.status(10_100))
    }

    @Test
    fun theAwakeGraceRunsFromWhenTheLinkWasLastUp() {
        val grace = 300_000L
        val t = tracker()
        assertTrue("starting up", t.keepAwake(1_000, grace))
        t.received(Reply.PROFILE, 10_000)
        assertTrue("up", t.keepAwake(15_000, grace))

        // The daemon goes away after the show.
        assertTrue(t.keepAwake(10_000 + grace - 1, grace))
        assertFalse("all night", t.keepAwake(10_000 + grace, grace))

        // It comes back: awake again at once.
        t.received(Reply.PONG, 900_000)
        assertTrue(t.keepAwake(900_000, grace))
    }

    @Test
    fun aRefusedNameDoesNotKeepThePhoneAwake() {
        // The daemon answers, but the link never comes up (audit R5).
        val grace = 300_000L
        val t = tracker()
        t.received(Reply.UNKNOWN_NAME, 100)
        t.received(Reply.UNKNOWN_NAME, grace + 100)
        assertFalse(t.keepAwake(grace + 200, grace))
    }

    @Test
    fun repliesFromAnotherAddressAreReportedAsSuch() {
        val t = tracker()
        t.strayReplyReceived()
        assertEquals(connecting, t.status(4_999))
        assertEquals(LinkStatus(false, LinkProblem.OTHER_ADDRESS), t.status(5_000))
    }

    @Test
    fun silenceAfterBeingUpIsLostThenAnyReplyRecovers() {
        val t = tracker()
        t.received(Reply.PROFILE, 0)
        assertEquals(up, t.status(5_999))
        assertEquals(LinkStatus(false, LinkProblem.LOST), t.status(6_000))

        t.received(Reply.PONG, 20_000)
        assertEquals(up, t.status(20_000))
    }
}

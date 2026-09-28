package com.pob31.s21monitor.service

import com.pob31.s21monitor.model.LinkProblem
import org.junit.Assert.assertEquals
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
    fun silenceAfterBeingUpIsLostThenAnyReplyRecovers() {
        val t = tracker()
        t.received(Reply.PROFILE, 0)
        assertEquals(up, t.status(5_999))
        assertEquals(LinkStatus(false, LinkProblem.LOST), t.status(6_000))

        t.received(Reply.PONG, 20_000)
        assertEquals(up, t.status(20_000))
    }
}

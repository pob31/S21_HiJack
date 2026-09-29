package com.pob31.s21monitor.service

import com.pob31.s21monitor.model.LinkProblem

/** What the UI shows about the link (audit A7). */
data class LinkStatus(val connected: Boolean, val problem: LinkProblem?)

/** What kind of packet came back from the daemon. */
enum class Reply {
    /** The ping's answer: the daemon is reachable, nothing more. */
    PONG,
    /** State or names for this profile: the daemon knows it. */
    PROFILE,
    /** The daemon has no profile by this name. */
    UNKNOWN_NAME,
}

/**
 * Works out whether the link is up, and if not why, from what has come back
 * from the daemon and when (audit A7). Controls are live only while it's up.
 *
 * Thread-safe. Times are passed in (ms, one clock throughout) so tests can
 * drive them.
 */
class LinkTracker(
    private val startMs: Long,
    private val noReplyAfterMs: Long,
    private val timeoutMs: Long,
) {
    private var lastReplyMs: Long? = null
    private var knowsProfile = false
    private var unknownName = false
    /** When the link was last up (a reply while the profile is known). */
    private var lastUpMs: Long? = null
    /** A monitor reply came from an address other than the daemon's. */
    private var strayReply = false

    @Synchronized
    fun received(reply: Reply, nowMs: Long) {
        lastReplyMs = nowMs
        when (reply) {
            Reply.PONG -> {}
            Reply.PROFILE -> {
                knowsProfile = true
                unknownName = false
            }
            Reply.UNKNOWN_NAME -> {
                knowsProfile = false
                unknownName = true
            }
        }
        if (knowsProfile) lastUpMs = nowMs
    }

    /** A monitor reply arrived from another address than the daemon's: a
     *  daemon with several addresses answering from a different one. Its
     *  packets are still dropped, but "no reply" would mislead. */
    @Synchronized
    fun strayReplyReceived() {
        strayReply = true
    }

    /**
     * Whether the link should keep the phone awake (wake lock, Wi-Fi lock,
     * fast pings): while it's up, and for [graceMs] after it was last up or
     * began. A phone left on "No reply", "Name not recognised" or "Lost"
     * after the show used to hold both locks and ping every 2 s all night
     * (audit R5). Any reply that brings the link back makes this true again.
     */
    @Synchronized
    fun keepAwake(nowMs: Long, graceMs: Long): Boolean =
        status(nowMs).connected || nowMs - (lastUpMs ?: startMs) < graceMs

    @Synchronized
    fun status(nowMs: Long): LinkStatus {
        val last = lastReplyMs
        return when {
            last == null -> LinkStatus(
                false,
                when {
                    nowMs - startMs < noReplyAfterMs -> null
                    strayReply -> LinkProblem.OTHER_ADDRESS
                    else -> LinkProblem.NO_REPLY
                },
            )
            nowMs - last >= timeoutMs -> LinkStatus(false, LinkProblem.LOST)
            unknownName -> LinkStatus(false, LinkProblem.UNKNOWN_NAME)
            // Reachable, but no word about this profile yet: still connecting.
            else -> LinkStatus(knowsProfile, null)
        }
    }
}

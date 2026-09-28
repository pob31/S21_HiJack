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
    }

    @Synchronized
    fun status(nowMs: Long): LinkStatus {
        val last = lastReplyMs
        return when {
            last == null -> LinkStatus(
                false,
                if (nowMs - startMs >= noReplyAfterMs) LinkProblem.NO_REPLY else null,
            )
            nowMs - last >= timeoutMs -> LinkStatus(false, LinkProblem.LOST)
            unknownName -> LinkStatus(false, LinkProblem.UNKNOWN_NAME)
            // Reachable, but no word about this profile yet: still connecting.
            else -> LinkStatus(knowsProfile, null)
        }
    }
}

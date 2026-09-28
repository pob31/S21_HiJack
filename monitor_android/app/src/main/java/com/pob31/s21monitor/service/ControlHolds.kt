package com.pob31.s21monitor.service

/** One control on this phone. */
sealed interface Control {
    data class SendLevel(val input: Int, val aux: Int) : Control
    data class SendPan(val input: Int, val aux: Int) : Control
    data class SendOn(val input: Int, val aux: Int) : Control
    data class AuxFader(val aux: Int) : Control
    data class AuxMute(val aux: Int) : Control
}

/**
 * Controls whose incoming values are ignored so they don't move under the
 * musician's finger (audit A5): while a finger is on the control, and for
 * [holdMs] after it lifts or after the last change made on this phone.
 *
 * The daemon pushes every mirror change back to every client, including the
 * one that made it, and each heartbeat brings a full snapshot. During a drag
 * those arrive ~50 ms stale and would pull the fader back.
 *
 * Thread-safe. Times are passed in (ms, any monotonic clock) so tests can
 * drive them.
 */
class ControlHolds(private val holdMs: Long) {

    private val touched = HashSet<Control>()
    private val heldUntil = HashMap<Control, Long>()

    @Synchronized
    fun touch(control: Control, down: Boolean, nowMs: Long) {
        if (down) {
            touched += control
        } else {
            touched -= control
            heldUntil[control] = nowMs + holdMs
        }
    }

    /** A value for [control] was just set on this phone. */
    @Synchronized
    fun changed(control: Control, nowMs: Long) {
        heldUntil[control] = nowMs + holdMs
    }

    @Synchronized
    fun isHeld(control: Control, nowMs: Long): Boolean =
        control in touched || (heldUntil[control]?.let { nowMs < it } ?: false)

    /** [local] while [control] is held, else [incoming]. */
    fun <T> pick(control: Control, nowMs: Long, local: T, incoming: T): T =
        if (isHeld(control, nowMs)) local else incoming
}

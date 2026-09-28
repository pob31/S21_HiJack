package com.pob31.s21monitor.model

/** Connection details for a monitoring profile (the native/UDP path is
 *  name-only — PINs are web-only). Persisted via CredentialsStore. */
data class Credentials(
    val name: String,
    val host: String,
    val port: Int,
)

/** One input→aux send the musician can control. dB level, −1..+1 pan, on/off. */
data class SendState(
    val input: Int,
    val aux: Int,
    val level: Float = OFF_DB,
    val pan: Float = 0f,
    val on: Boolean = false,
    val name: String = "",
)

/** One aux master strip. */
data class AuxState(
    val aux: Int,
    val fader: Float = OFF_DB,
    val mute: Boolean = false,
    val name: String = "",
)

/** Why the link to the daemon isn't up, when it isn't (audit A7). */
enum class LinkProblem {
    /** Nothing has come back from the daemon's address since the link began. */
    NO_REPLY,
    /** The daemon replied that it has no monitor profile by this name. */
    UNKNOWN_NAME,
    /** The daemon was answering, and has stopped. */
    LOST,
}

/** Immutable snapshot the UI renders. Replaced wholesale on each change so
 *  Compose recomposes from a single StateFlow. */
data class MonitorUiState(
    /** The daemon is answering and knows this profile; controls are live. */
    val connected: Boolean = false,
    val problem: LinkProblem? = null,
    val console: String = "",
    val sends: Map<Pair<Int, Int>, SendState> = emptyMap(), // key = (input, aux)
    val auxes: Map<Int, AuxState> = emptyMap(),
) {
    /** Auxes this client controls — union of strip + send auxes, sorted. */
    val availableAuxes: List<Int>
        get() = (auxes.keys + sends.keys.map { it.second }).distinct().sorted()

    fun sendsForAux(aux: Int): List<SendState> =
        sends.values.filter { it.aux == aux }.sortedBy { it.input }

    /** This state with another client's echoed change applied to the send
     *  (input, aux). An echo for a send this client doesn't already have is
     *  ignored: it would add a strip for an input this musician can't see,
     *  in among their own strips (audit A1). */
    fun withSendEcho(input: Int, aux: Int, transform: (SendState) -> SendState): MonitorUiState {
        val key = input to aux
        val send = sends[key] ?: return this
        return copy(sends = sends + (key to transform(send)))
    }
}

/** Inaudible floor / "off" sentinel, matching the daemon + Flutter client. */
const val OFF_DB = -150f

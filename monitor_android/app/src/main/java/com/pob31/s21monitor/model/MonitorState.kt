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

/** Immutable snapshot the UI renders. Replaced wholesale on each change so
 *  Compose recomposes from a single StateFlow. */
data class MonitorUiState(
    val connected: Boolean = false,
    val console: String = "",
    val sends: Map<Pair<Int, Int>, SendState> = emptyMap(), // key = (input, aux)
    val auxes: Map<Int, AuxState> = emptyMap(),
) {
    /** Auxes this client controls — union of strip + send auxes, sorted. */
    val availableAuxes: List<Int>
        get() = (auxes.keys + sends.keys.map { it.second }).distinct().sorted()

    fun sendsForAux(aux: Int): List<SendState> =
        sends.values.filter { it.aux == aux }.sortedBy { it.input }
}

/** Inaudible floor / "off" sentinel, matching the daemon + Flutter client. */
const val OFF_DB = -150f

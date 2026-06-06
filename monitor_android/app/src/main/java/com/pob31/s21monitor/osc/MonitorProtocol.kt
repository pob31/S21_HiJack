package com.pob31.s21monitor.osc

/**
 * The `/monitor/...` wire contract, shared with the daemon and the web client.
 *
 * Outbound (client → daemon): heartbeat/connect, state request, and the
 * send/aux setters. Inbound (daemon → client): full send state, per-param
 * echoes of other clients' moves, aux strips, channel names, discovery reply.
 */
object MonitorProtocol {

    // ── Outbound addresses ──
    fun connect(name: String) = "/monitor/$name/connect"
    fun requestState(name: String) = "/monitor/$name/state"
    fun sendLevel(name: String, input: Int, aux: Int) = "/monitor/$name/send/$input/$aux/level"
    fun sendPan(name: String, input: Int, aux: Int) = "/monitor/$name/send/$input/$aux/pan"
    fun sendOn(name: String, input: Int, aux: Int) = "/monitor/$name/send/$input/$aux/on"
    fun auxFader(name: String, aux: Int) = "/monitor/$name/aux/$aux/fader"
    fun auxMute(name: String, aux: Int) = "/monitor/$name/aux/$aux/mute"
    const val DISCOVER = "/monitor/discover"

    // ── Inbound, parsed ──
    sealed interface Inbound {
        /** `/monitor/state/send/{in}/{aux}` — full send state. */
        data class SendFull(val input: Int, val aux: Int, val level: Float, val pan: Float, val on: Boolean) : Inbound
        /** `/monitor/state/send/{in}/{aux}/{level|pan|on}` — echo of another client's change. */
        data class SendEcho(val input: Int, val aux: Int, val param: String, val arg: OscArg) : Inbound
        /** `/monitor/state/aux/{aux}` — aux master strip. */
        data class AuxStrip(val aux: Int, val fader: Float, val mute: Boolean) : Inbound
        /** `/monitor/state/name/input/{in}`. */
        data class NameInput(val input: Int, val name: String) : Inbound
        /** `/monitor/state/name/aux/{aux}`. */
        data class NameAux(val aux: Int, val name: String) : Inbound
        /** `/monitor/discovered` `s i` — console name + daemon port. */
        data class Discovered(val console: String, val port: Int) : Inbound
    }

    fun parse(msg: OscMessage): Inbound? {
        val a = msg.args
        val p = msg.address.split('/') // ["", "monitor", "state", ...]

        if (msg.address == "/monitor/discovered") {
            val console = (a.getOrNull(0) as? OscString)?.value ?: return null
            val port = (a.getOrNull(1) as? OscInt)?.value ?: 0
            return Inbound.Discovered(console, port)
        }
        if (p.size < 4 || p[1] != "monitor" || p[2] != "state") return null

        return when (p[3]) {
            "send" -> {
                val input = p.getOrNull(4)?.toIntOrNull() ?: return null
                val aux = p.getOrNull(5)?.toIntOrNull() ?: return null
                if (p.size >= 7) {
                    // echo: /monitor/state/send/{in}/{aux}/{suffix}
                    val arg = a.getOrNull(0) ?: return null
                    Inbound.SendEcho(input, aux, p[6], arg)
                } else if (a.size >= 3) {
                    Inbound.SendFull(input, aux, asFloat(a[0]), asFloat(a[1]), asBool(a[2]))
                } else null
            }
            "aux" -> {
                val aux = p.getOrNull(4)?.toIntOrNull() ?: return null
                if (a.size < 2) return null
                Inbound.AuxStrip(aux, asFloat(a[0]), asBool(a[1]))
            }
            "name" -> {
                val kind = p.getOrNull(4) ?: return null
                val ch = p.getOrNull(5)?.toIntOrNull() ?: return null
                val name = (a.getOrNull(0) as? OscString)?.value ?: return null
                when (kind) {
                    "input" -> Inbound.NameInput(ch, name)
                    "aux" -> Inbound.NameAux(ch, name)
                    else -> null
                }
            }
            else -> null
        }
    }

    /** Coerce an OSC arg to a float (the daemon may echo on/level as f/i). */
    fun asFloat(arg: OscArg): Float = when (arg) {
        is OscFloat -> arg.value
        is OscInt -> arg.value.toFloat()
        is OscBool -> if (arg.value) 1f else 0f
        is OscString -> arg.value.toFloatOrNull() ?: 0f
    }

    /** Coerce an OSC arg to a bool (echoes may arrive as T/F, int, or float). */
    fun asBool(arg: OscArg): Boolean = when (arg) {
        is OscBool -> arg.value
        is OscInt -> arg.value != 0
        is OscFloat -> arg.value != 0f
        is OscString -> arg.value == "1" || arg.value.equals("true", true)
    }
}

package com.pob31.s21monitor.osc

import java.io.ByteArrayOutputStream

/** OSC argument types used by the monitor protocol. */
sealed class OscArg
data class OscFloat(val value: Float) : OscArg()
data class OscInt(val value: Int) : OscArg()
data class OscString(val value: String) : OscArg()
data class OscBool(val value: Boolean) : OscArg()

/** A decoded OSC message. [sourceHost]/[sourcePort] are set for received packets. */
data class OscMessage(
    val address: String,
    val args: List<OscArg> = emptyList(),
    val sourceHost: String? = null,
    val sourcePort: Int? = null,
)

/**
 * Minimal big-endian OSC 1.0 encoder/decoder for the monitor protocol — a
 * direct Kotlin port of the Flutter client's `osc_service.dart`, so the two
 * clients are byte-compatible with the daemon. Strings are null-terminated
 * and padded to a 4-byte boundary; `T`/`F` (bool) carry no argument bytes.
 */
object OscCodec {

    fun encode(address: String, args: List<OscArg>): ByteArray {
        val out = ByteArrayOutputStream()
        out.write(encodeString(address))

        val types = StringBuilder(",")
        for (a in args) {
            types.append(
                when (a) {
                    is OscFloat -> 'f'
                    is OscInt -> 'i'
                    is OscString -> 's'
                    is OscBool -> if (a.value) 'T' else 'F'
                }
            )
        }
        out.write(encodeString(types.toString()))

        for (a in args) {
            when (a) {
                is OscFloat -> out.write(int32(java.lang.Float.floatToIntBits(a.value)))
                is OscInt -> out.write(int32(a.value))
                is OscString -> out.write(encodeString(a.value))
                is OscBool -> { /* T/F have no payload */ }
            }
        }
        return out.toByteArray()
    }

    fun decode(data: ByteArray, length: Int = data.size): OscMessage? {
        return try {
            var off = 0
            val (address, a1) = readString(data, off, length); off = a1
            val (typeTag, a2) = readString(data, off, length); off = a2
            if (!typeTag.startsWith(",")) return null

            val args = ArrayList<OscArg>()
            for (t in typeTag.substring(1)) {
                when (t) {
                    'f' -> { args.add(OscFloat(java.lang.Float.intBitsToFloat(readInt32(data, off)))); off += 4 }
                    'i' -> { args.add(OscInt(readInt32(data, off))); off += 4 }
                    's' -> { val (s, e) = readString(data, off, length); args.add(OscString(s)); off = e }
                    'T' -> args.add(OscBool(true))
                    'F' -> args.add(OscBool(false))
                    else -> { /* skip unknown */ }
                }
            }
            OscMessage(address, args)
        } catch (e: Exception) {
            null
        }
    }

    // ── helpers ──

    private fun encodeString(s: String): ByteArray {
        val raw = s.toByteArray(Charsets.UTF_8)
        val padded = (raw.size + 1 + 3) and 3.inv() // +1 for null, round up to 4
        val buf = ByteArray(padded)
        System.arraycopy(raw, 0, buf, 0, raw.size)
        // remaining bytes are already 0 (null terminator + padding)
        return buf
    }

    private fun int32(v: Int): ByteArray = byteArrayOf(
        (v ushr 24).toByte(),
        (v ushr 16).toByte(),
        (v ushr 8).toByte(),
        v.toByte(),
    )

    private fun readInt32(data: ByteArray, off: Int): Int =
        ((data[off].toInt() and 0xFF) shl 24) or
            ((data[off + 1].toInt() and 0xFF) shl 16) or
            ((data[off + 2].toInt() and 0xFF) shl 8) or
            (data[off + 3].toInt() and 0xFF)

    private fun readString(data: ByteArray, off: Int, length: Int): Pair<String, Int> {
        var end = off
        while (end < length && data[end].toInt() != 0) end++
        val s = String(data, off, end - off, Charsets.UTF_8)
        var next = end + 1 // skip null
        while (next % 4 != 0) next++ // skip padding
        return s to next
    }
}

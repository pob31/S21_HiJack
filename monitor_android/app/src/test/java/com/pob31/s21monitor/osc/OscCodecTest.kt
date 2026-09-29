package com.pob31.s21monitor.osc

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Audit A12: argument types this client doesn't use keep the rest aligned. */
class OscCodecTest {

    /** An OSC message with raw [payload] bytes after the type tags. */
    private fun message(typeTags: String, vararg payload: Int): ByteArray {
        fun padded(s: String): List<Int> {
            val bytes = s.toByteArray().map { it.toInt() } + 0
            return bytes + List((4 - bytes.size % 4) % 4) { 0 }
        }
        return (padded("/x") + padded(typeTags) + payload.toList())
            .map { it.toByte() }.toByteArray()
    }

    private val seven = intArrayOf(0, 0, 0, 7)

    @Test
    fun aFixedSizeArgumentIsSteppedOver() {
        val int64 = IntArray(8)
        val msg = OscCodec.decode(message(",hi", *int64, *seven))
        assertEquals(listOf<OscArg>(OscInt(7)), msg?.args)
    }

    @Test
    fun aBlobIsSteppedOverWithItsPadding() {
        // Five bytes of blob, padded to eight.
        val blob = intArrayOf(0, 0, 0, 5, 1, 2, 3, 4, 5, 0, 0, 0)
        val msg = OscCodec.decode(message(",bi", *blob, *seven))
        assertEquals(listOf<OscArg>(OscInt(7)), msg?.args)
    }

    @Test
    fun aTypeOfUnknownSizeEndsTheDecode() {
        assertNull(OscCodec.decode(message(",?i", *seven)))
    }

    @Test
    fun theTypesInUseStillDecode() {
        val msg = OscCodec.decode(OscCodec.encode("/x", listOf(OscInt(3), OscBool(true))))
        assertEquals(listOf<OscArg>(OscInt(3), OscBool(true)), msg?.args)
    }
}

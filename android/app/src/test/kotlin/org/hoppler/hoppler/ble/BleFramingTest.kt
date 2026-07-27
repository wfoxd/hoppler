package org.hoppler.hoppler.ble

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The adapter's radio-free logic.
 *
 * These exist because BLE's untestable reputation was being allowed to cover
 * code that is not untestable at all: two of the three defects review found in
 * the adapter were a byte-array comparison and a missing input check, neither
 * of which needs a radio to demonstrate. What genuinely needs two phones is in
 * `BleAdapter`; everything here runs in CI.
 */
class BleFramingTest {

    private fun bytes(vararg v: Int) = ByteArray(v.size) { v[it].toByte() }

    // ── the codec ───────────────────────────────────────────────────────────

    @Test
    fun `a framed advertisement decodes back to what went in`() {
        val payload = bytes(0xDE, 0xAD, 0xBE, 0xEF)
        val decoded = BleFraming.unframe(BleFraming.frame(0x1234, "node-a", payload))
        assertEquals(BleFraming.Advert("node-a", 0x1234, payload), decoded)
    }

    @Test
    fun `an empty payload is a valid advertisement`() {
        // Not a corner case: a peer advertising presence with no persona yet is
        // exactly this. It must not be confused with a malformed frame.
        val decoded = BleFraming.unframe(BleFraming.frame(0x25, "n", ByteArray(0)))
        assertEquals("n", decoded?.id)
        assertArrayEquals(ByteArray(0), decoded?.payload)
    }

    @Test
    fun `a psm using the high byte survives the round trip`() {
        // A PSM is 16-bit and real ones exceed 255; truncating to one byte
        // would dial the wrong channel and present as an unreachable peer.
        assertEquals(0xFF80, BleFraming.unframe(BleFraming.frame(0xFF80, "n", ByteArray(0)))?.psm)
    }

    @Test
    fun `the longest permitted id round trips`() {
        val id = "a".repeat(BleFraming.MAX_ID)
        assertEquals(id, BleFraming.unframe(BleFraming.frame(1, id, bytes(1)))?.id)
    }

    @Test
    fun `framing rejects an id the wire format cannot carry`() {
        // The length prefix is one byte, so a 64-byte id would silently wrap.
        for (bad in listOf("", "a".repeat(BleFraming.MAX_ID + 1))) {
            try {
                BleFraming.frame(1, bad, ByteArray(0))
                throw AssertionError("framed an id of length ${bad.length}")
            } catch (expected: IllegalArgumentException) {
                // correct
            }
        }
    }

    // ── hostile input ───────────────────────────────────────────────────────

    @Test
    fun `a truncated frame is refused rather than read past its end`() {
        // Anyone in radio range can broadcast these, so every length must be
        // checked against the buffer instead of trusted.
        for (bad in listOf(
            ByteArray(0),
            bytes(0x12),
            bytes(0x12, 0x34), // header only, no length byte
            bytes(0x12, 0x34, 0x05, 0x61), // claims 5 id bytes, carries 1
            bytes(0x12, 0x34, 0xFF) // claims 255, carries none
        )) {
            assertNull("accepted a truncated frame of ${bad.size} bytes", BleFraming.unframe(bad))
        }
    }

    @Test
    fun `a zero length id is refused`() {
        assertNull(BleFraming.unframe(bytes(0x12, 0x34, 0x00, 0x61, 0x62)))
    }

    @Test
    fun `an id that breaks the core's rule never decodes`() {
        // The core would reject these anyway — but not before they had entered
        // the adapter's own peer map, which is unbounded and reachable by any
        // stranger in range.
        for (bad in listOf("has.dot", "has:colon", "-lead", "trail-", "sp ace", "sla/sh")) {
            val frame = bytes(0x00, 0x25, bad.length) + bad.toByteArray(Charsets.US_ASCII)
            assertNull("accepted invalid id '$bad'", BleFraming.unframe(frame))
        }
    }

    @Test
    fun `the id rule matches the core's`() {
        for (good in listOf("a", "node-a", "A1", "0", "a-b-c", "a".repeat(63))) {
            assertTrue("rejected valid id '$good'", BleFraming.isValidPeerId(good))
        }
        for (bad in listOf("", "-a", "a-", "a.b", "a:b", "a b", "a".repeat(64), "é")) {
            assertFalse("accepted invalid id '$bad'", BleFraming.isValidPeerId(bad))
        }
    }

    @Test
    fun `a frame carrying someone else's service data is refused, not guessed at`() {
        // Our UUID filters most of this, but the scan filter is best-effort on
        // some stacks and the payload is attacker-controlled regardless.
        assertNull(BleFraming.unframe(bytes(0xFF, 0xFF, 0x02, 0x00, 0x01)))
    }

    // ── the sighting decision ───────────────────────────────────────────────

    @Test
    fun `a first sighting is news`() {
        assertTrue(BleFraming.isNews(null, BleFraming.Advert("a", 1, bytes(1))))
    }

    @Test
    fun `an unchanged repeat is not news`() {
        // Advertisements repeat several times a second; forwarding every one
        // would wake the core continuously for no new information.
        val advert = BleFraming.Advert("a", 1, bytes(1, 2))
        assertFalse(BleFraming.isNews(advert, BleFraming.Advert("a", 1, bytes(1, 2))))
    }

    @Test
    fun `a changed payload is news`() {
        // The regression that matters: a payload change is the peer's persona
        // changing. Suppressing it leaves every scanner in range showing a
        // stale name and colour, with no later event to correct it.
        val before = BleFraming.Advert("a", 1, bytes(1, 2))
        assertTrue(BleFraming.isNews(before, BleFraming.Advert("a", 1, bytes(1, 3))))
        assertTrue(BleFraming.isNews(before, BleFraming.Advert("a", 1, bytes(1))))
        assertTrue(BleFraming.isNews(before, BleFraming.Advert("a", 1, ByteArray(0))))
    }

    @Test
    fun `a changed psm is news`() {
        // The peer restarted its listener; the old PSM now dials nothing.
        val before = BleFraming.Advert("a", 1, bytes(1))
        assertTrue(BleFraming.isNews(before, BleFraming.Advert("a", 2, bytes(1))))
    }

    @Test
    fun `payload equality is by content, not identity`() {
        // A generated data-class equals() compares arrays by reference, which
        // would make every repeat look like a change — the same bug inverted.
        val a = BleFraming.Advert("a", 1, bytes(1, 2, 3))
        val b = BleFraming.Advert("a", 1, bytes(1, 2, 3))
        assertEquals(a, b)
        assertEquals(a.hashCode(), b.hashCode())
        assertFalse(BleFraming.isNews(a, b))
    }
}

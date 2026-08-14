package org.hoppler.hoppler.nfc

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The tap protocol, on the JVM. Two phones can only tell you whether a tap
 * worked; these say *why* when it does not.
 */
class NfcApduTest {

    /** The reader's own SELECT must be one this adapter would answer — the two
     * halves of the exchange live in one file precisely so they cannot drift,
     * and this is what holds them together. */
    @Test
    fun the_select_we_send_is_the_select_we_recognise() {
        assertTrue(NfcApdu.isSelect(NfcApdu.selectCommand()))
    }

    @Test
    fun a_select_for_someone_elses_application_is_not_ours() {
        val other = byteArrayOf(0xF0.toByte(), 1, 2, 3, 4, 5, 6, 7)
        assertFalse(NfcApdu.isSelect(NfcApdu.selectCommand(other)))
        // ...and a truncated one is not ours either, rather than matching on a
        // prefix. A reader that got cut off mid-AID has not selected us.
        val short = NfcApdu.selectCommand().copyOfRange(0, 9)
        assertFalse(NfcApdu.isSelect(short))
    }

    @Test
    fun junk_is_not_mistaken_for_a_command() {
        for (junk in listOf(
            ByteArray(0),
            byteArrayOf(0x00),
            byteArrayOf(0x00, 0xA4.toByte()),
            byteArrayOf(0x00, 0xB0.toByte(), 0x04, 0x00, 0x08),
        )) {
            assertFalse("accepted ${junk.size} bytes of junk as SELECT", NfcApdu.isSelect(junk))
            assertFalse(NfcApdu.isGetResponse(junk))
        }
    }

    /** A code that fits goes in one response, with a clean status. */
    @Test
    fun a_short_payload_arrives_whole_in_one_response() {
        val payload = "HOPPLER://PAIR/MZXW6YTBOI".toByteArray()
        val response = NfcApdu.respond(payload, 0)
        assertEquals(0, response.remaining)
        assertTrue(NfcApdu.isOk(response.bytes))
        assertArrayEquals(payload, NfcApdu.body(response.bytes))
    }

    /**
     * The case this chunking exists for.
     *
     * An invite runs about 160 bytes, but its rung hint is bounded at 64 and a
     * full one pushes the code past what a single short-form response carries.
     * Sent whole it would lose its tail, and arrive as a code that simply does
     * not parse — on the screen where nobody can see why.
     */
    @Test
    fun a_long_payload_is_reassembled_exactly() {
        val payload = ByteArray(600) { (it % 251).toByte() }
        val gathered = ArrayList<Byte>()
        var offset = 0
        var rounds = 0
        while (true) {
            val response = NfcApdu.respond(payload, offset)
            gathered.addAll(NfcApdu.body(response.bytes).toList())
            offset += NfcApdu.body(response.bytes).size
            rounds++
            if (response.remaining == 0) {
                assertTrue("the last response was not a clean 90 00", NfcApdu.isOk(response.bytes))
                break
            }
            assertTrue("a partial response did not say so", NfcApdu.hasMore(response.bytes))
            check(rounds < 20) { "chunking did not terminate" }
        }
        assertArrayEquals(payload, gathered.toByteArray())
        assertTrue("a 600-byte payload came back in one response", rounds > 1)
    }

    /** A reader that asks for fewer bytes gets fewer, and still gets all of
     * them in the end. Readers do ask: `Le` is theirs to choose. */
    @Test
    fun a_reader_asking_for_small_chunks_still_gets_everything() {
        val payload = ByteArray(300) { it.toByte() }
        val gathered = ArrayList<Byte>()
        var offset = 0
        while (offset < payload.size) {
            val response = NfcApdu.respond(payload, offset, want = 16)
            val body = NfcApdu.body(response.bytes)
            assertTrue(body.size <= 16)
            gathered.addAll(body.toList())
            offset += body.size
        }
        assertArrayEquals(payload, gathered.toByteArray())
    }

    /** Past the end is a clean finish, not an empty chunk forever. */
    @Test
    fun asking_past_the_end_ends_the_exchange() {
        val payload = "short".toByteArray()
        val response = NfcApdu.respond(payload, payload.size)
        assertEquals(0, response.remaining)
        assertTrue(NfcApdu.isOk(response.bytes))
        assertEquals(0, NfcApdu.body(response.bytes).size)
    }

    /** No response body may exceed what a short-form APDU can carry, whatever
     * the reader asks for — including a reader that asks for more than the
     * field can express. */
    @Test
    fun a_response_never_outgrows_a_short_apdu() {
        val payload = ByteArray(1000)
        for (want in listOf(1, 16, NfcApdu.MAX_BODY, 254, 255, 1000)) {
            val response = NfcApdu.respond(payload, 0, want)
            assertTrue(
                "a response of ${response.bytes.size} bytes for want=$want",
                response.bytes.size <= 255,
            )
        }
    }

    @Test
    fun get_response_length_is_read_and_bounded() {
        assertEquals(16, NfcApdu.requestedLength(NfcApdu.getResponseCommand(16)))
        // `00` means 256 in ISO 7816, which is more than a short response can
        // hold; clamped rather than wrapped to zero, which would stall.
        assertEquals(NfcApdu.MAX_BODY, NfcApdu.requestedLength(NfcApdu.getResponseCommand(0)))
        assertEquals(NfcApdu.MAX_BODY, NfcApdu.requestedLength(byteArrayOf(0x00, 0xC0.toByte())))
    }

    /** The status words are wire. A reader on an older build has to keep
     * telling these apart. */
    @Test
    fun the_status_words_are_fixed() {
        assertArrayEquals(byteArrayOf(0x90.toByte(), 0x00), NfcApdu.OK)
        assertArrayEquals(byteArrayOf(0x6A, 0x82.toByte()), NfcApdu.NOT_FOUND)
        assertArrayEquals(byteArrayOf(0x69, 0x85.toByte()), NfcApdu.NOTHING_TO_SHARE)
        assertFalse(NfcApdu.isOk(NfcApdu.NOT_FOUND))
        assertFalse(NfcApdu.isOk(NfcApdu.NOTHING_TO_SHARE))
    }

    /** The AID is wire too: change it and a new phone cannot read an old one. */
    @Test
    fun the_aid_is_fixed_and_proprietary() {
        assertEquals(8, NfcApdu.AID.size)
        assertEquals(
            "f0484f50504c4552",
            NfcApdu.AID.joinToString("") { "%02x".format(it) },
        )
        // ISO 7816-5 leaves the `F` range to unregistered applications. Anything
        // else would be squatting on an issuer's identifier.
        assertEquals(0xF0, NfcApdu.AID[0].toInt() and 0xF0)
    }
}

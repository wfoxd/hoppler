package org.hoppler.hoppler.nfc

/**
 * The ISO 7816 exchange a pairing tap consists of — the part that needs no
 * radio.
 *
 * Split out of the adapter for the same reason as [org.hoppler.hoppler.ble.BleFraming]:
 * NFC behaviour can only be judged with two phones touching, but *parsing a
 * command APDU* and *cutting a payload into responses* are ordinary functions,
 * and they are where the silent bugs live. Everything here runs on the JVM in
 * CI; what stays in the adapter is the part that genuinely needs hardware.
 *
 * # Why an APDU exchange and not a tap-to-beam
 *
 * Android Beam — the NDEF push that "tap two phones together" used to mean —
 * was deprecated in Android 10 and **removed in Android 14**. There is no
 * phone-to-phone NDEF push left on any device this project supports (minSdk 31,
 * both test handsets on 14+).
 *
 * What remains is card emulation: the phone showing the code registers a
 * [android.nfc.cardemulation.HostApduService] under an AID, and the phone
 * reading it uses reader mode, connects with `IsoDep`, and talks to it as if it
 * were a smartcard. So the "tap" is a reader and a card, and the card is the
 * person holding their code up — which is also why the roles match the QR path
 * exactly: whoever shows, shows; whoever reads, reads.
 */
object NfcApdu {

    /**
     * The application identifier a reader selects.
     *
     * Sixteen hex digits in the `F0…` range, which ISO 7816-5 leaves to
     * proprietary use — an unregistered application must not squat on an AID
     * that belongs to a registered issuer, and `F` is the prefix set aside for
     * exactly this. The bytes after it are arbitrary and fixed here; changing
     * them makes new phones unable to read old ones, so they are wire.
     */
    val AID: ByteArray = byteArrayOf(
        0xF0.toByte(), 0x48, 0x4F, 0x50, 0x50, 0x4C, 0x45, 0x52,
    )

    /** `90 00` — done, nothing more to fetch. */
    val OK: ByteArray = byteArrayOf(0x90.toByte(), 0x00)

    /** `6A 82` — no such file: the reader selected something we do not serve. */
    val NOT_FOUND: ByteArray = byteArrayOf(0x6A, 0x82.toByte())

    /** `69 85` — conditions not satisfied: selected us, but there is no code on
     * screen to hand over. Distinct from [NOT_FOUND] so the reader can tell
     * "wrong app" from "right app, nothing to give". */
    val NOTHING_TO_SHARE: ByteArray = byteArrayOf(0x69, 0x85.toByte())

    /** `6F 00` — anything we did not understand. */
    val UNKNOWN: ByteArray = byteArrayOf(0x6F, 0x00)

    /** Largest payload body in one response, leaving room for the status word. */
    const val MAX_BODY = 253

    /** `00 A4 04 00 <len> <aid>` — SELECT by name. */
    fun selectCommand(aid: ByteArray = AID): ByteArray =
        byteArrayOf(0x00, 0xA4.toByte(), 0x04, 0x00, aid.size.toByte()) + aid

    /** `00 C0 00 00 <len>` — GET RESPONSE, the fetch for everything after the
     * first chunk. */
    fun getResponseCommand(length: Int): ByteArray =
        byteArrayOf(0x00, 0xC0.toByte(), 0x00, 0x00, length.coerceIn(0, 255).toByte())

    /** Whether this command selects [aid]. */
    fun isSelect(command: ByteArray, aid: ByteArray = AID): Boolean {
        if (command.size < 5) return false
        if (command[0] != 0x00.toByte() || command[1] != 0xA4.toByte()) return false
        if (command[2] != 0x04.toByte() || command[3] != 0x00.toByte()) return false
        val len = command[4].toInt() and 0xFF
        if (command.size < 5 + len) return false
        return command.copyOfRange(5, 5 + len).contentEquals(aid)
    }

    fun isGetResponse(command: ByteArray): Boolean =
        command.size >= 4 && command[0] == 0x00.toByte() && command[1] == 0xC0.toByte()

    /** How many bytes the reader asked for, or [MAX_BODY] if it did not say. */
    fun requestedLength(command: ByteArray): Int {
        if (!isGetResponse(command) || command.size < 5) return MAX_BODY
        val asked = command[4].toInt() and 0xFF
        return if (asked == 0) MAX_BODY else asked.coerceAtMost(MAX_BODY)
    }

    /** One response: the bytes to send, and how much of [payload] is left. */
    data class Response(val bytes: ByteArray, val remaining: Int) {
        override fun equals(other: Any?): Boolean =
            this === other ||
                (other is Response && bytes.contentEquals(other.bytes) && remaining == other.remaining)

        override fun hashCode(): Int = bytes.contentHashCode() * 31 + remaining
    }

    /**
     * Cut one response out of [payload] starting at [offset].
     *
     * Chunked, and **the chunking is not exercised today**. That was worth
     * measuring rather than asserting: an invite is 126 bytes with no rung
     * hint and 231 with the longest hint the core allows (64 bytes,
     * `MAX_BLE_HINT_LEN`), against the [MAX_BODY] of 253 one response carries.
     * So every invite this build can produce fits in one. An earlier version
     * of this comment claimed a full hint pushed past the limit. It does not.
     *
     * Kept anyway, because the failure it prevents is the expensive kind: a
     * payload that silently lost its tail arrives as a code that does not
     * parse, on the one screen where nobody can see why. The invite is a wire
     * format that can grow, and the day it does this is either already right
     * or a bug nobody will look for.
     *
     * The measurement is pinned by `an_invite_fits_one_nfc_response` in
     * `rust/src/pairing/invite.rs` — on the Rust side, because that is where
     * the length is decided.
     *
     * More to come is signalled with `61 XX`, ISO 7816's "XX bytes still
     * waiting", which is what makes the reader ask again with GET RESPONSE.
     */
    fun respond(payload: ByteArray, offset: Int, want: Int = MAX_BODY): Response {
        if (offset >= payload.size) return Response(OK, 0)
        val take = minOf(want.coerceIn(1, MAX_BODY), payload.size - offset)
        val body = payload.copyOfRange(offset, offset + take)
        val left = payload.size - offset - take
        val status = if (left == 0) {
            OK
        } else {
            // `61 XX`, where XX is how much is left — capped at 255 because the
            // field is one byte, and a reader that asks for 255 simply comes
            // back again for the rest.
            byteArrayOf(0x61, left.coerceAtMost(255).toByte())
        }
        return Response(body + status, left)
    }

    /** Whether a response says more is waiting. */
    fun hasMore(response: ByteArray): Boolean =
        response.size >= 2 && response[response.size - 2] == 0x61.toByte()

    /** Whether a response ended cleanly. */
    fun isOk(response: ByteArray): Boolean =
        response.size >= 2 &&
            response[response.size - 2] == 0x90.toByte() &&
            response[response.size - 1] == 0x00.toByte()

    /** The payload half of a response, without its two status bytes. */
    fun body(response: ByteArray): ByteArray =
        if (response.size <= 2) ByteArray(0) else response.copyOfRange(0, response.size - 2)
}

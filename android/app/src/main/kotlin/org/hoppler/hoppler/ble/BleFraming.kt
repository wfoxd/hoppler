package org.hoppler.hoppler.ble

/**
 * The advertisement codec and the sighting decision — the adapter's logic that
 * needs no radio.
 *
 * Split out of [BleAdapter] deliberately. BLE behaviour can only be verified on
 * two phones, but *parsing a byte array* and *deciding whether a sighting is
 * news* are ordinary functions, and both classes of bug have already been found
 * here by review rather than by test. Everything in this file runs on the JVM
 * in CI; what stays in `BleAdapter` is the part that genuinely needs hardware.
 */
object BleFraming {

    /** Bytes of framing the adapter adds ahead of the core's payload. */
    const val OVERHEAD = 3

    /** Longest id the core's `is_valid_peer_id` permits. */
    const val MAX_ID = 63

    /** A decoded advertisement. */
    data class Advert(val id: String, val psm: Int, val payload: ByteArray) {
        // ByteArray needs content equality; the generated one compares refs.
        override fun equals(other: Any?): Boolean =
            this === other ||
                (other is Advert &&
                    id == other.id &&
                    psm == other.psm &&
                    payload.contentEquals(other.payload))

        override fun hashCode(): Int =
            (id.hashCode() * 31 + psm) * 31 + payload.contentHashCode()
    }

    /**
     * Service data is `[psm:2][idLen:1][id][payload]`.
     *
     * The PSM travels in the advertisement so a scanner can dial without a GATT
     * round-trip, and the id travels because the MAC address rotates underneath
     * us — a peer identified by MAC would change id mid-pipe and break contract
     * rule 4.
     */
    fun frame(psm: Int, localId: String, payload: ByteArray): ByteArray {
        val id = localId.toByteArray(Charsets.US_ASCII)
        require(id.size in 1..MAX_ID) { "local id must be 1..$MAX_ID bytes" }
        require(psm in 0..0xFFFF) { "psm must fit two bytes" }
        return ByteArray(OVERHEAD + id.size + payload.size).also {
            it[0] = (psm shr 8).toByte()
            it[1] = (psm and 0xFF).toByte()
            it[2] = id.size.toByte()
            id.copyInto(it, 2 + 1)
            payload.copyInto(it, OVERHEAD + id.size)
        }
    }

    /**
     * Decode an advertisement, or null if it is not one of ours.
     *
     * An advertisement is whatever a stranger chose to broadcast, so every
     * length here is checked against the buffer rather than trusted. The id
     * rule is enforced too: the core would reject a malformed id anyway, but an
     * unfiltered one still reaches the adapter's peer map on the way there, and
     * anyone in radio range could grow that map without limit.
     */
    fun unframe(data: ByteArray): Advert? {
        if (data.size < OVERHEAD) return null
        val psm = ((data[0].toInt() and 0xFF) shl 8) or (data[1].toInt() and 0xFF)
        val idLen = data[2].toInt() and 0xFF
        if (idLen == 0 || data.size < OVERHEAD + idLen) return null
        val id = String(data, OVERHEAD, idLen, Charsets.US_ASCII)
        if (!isValidPeerId(id)) return null
        return Advert(id, psm, data.copyOfRange(OVERHEAD + idLen, data.size))
    }

    /**
     * The core's rule, mirrored: ASCII alphanumeric and `-`, 1..63, no leading
     * or trailing `-`.
     *
     * Kept in step with `is_valid_peer_id` in `rust/src/transport/mod.rs`. Ids
     * are namespaced across rungs (`ble:1f3a`), so a `.` or `:` would split
     * differently on the two ends of one pipe — one connection under two names.
     */
    fun isValidPeerId(id: String): Boolean =
        id.isNotEmpty() &&
            id.length <= MAX_ID &&
            !id.startsWith('-') &&
            !id.endsWith('-') &&
            id.all { (it in 'a'..'z') || (it in 'A'..'Z') || (it in '0'..'9') || it == '-' }

    /**
     * Whether a fresh sighting is news worth telling the core.
     *
     * `PeerFound` is "latest known", so repeating an unchanged advertisement is
     * noise — but a changed payload is the peer's *persona* changing, and
     * suppressing it strands every scanner on a stale name and colour with no
     * event to correct it. That is the bug this function exists to make
     * testable.
     */
    fun isNews(previous: Advert?, current: Advert): Boolean =
        previous == null ||
            previous.psm != current.psm ||
            !previous.payload.contentEquals(current.payload)
}

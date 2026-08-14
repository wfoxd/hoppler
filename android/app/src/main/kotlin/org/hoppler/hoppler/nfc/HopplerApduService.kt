package org.hoppler.hoppler.nfc

import android.nfc.cardemulation.HostApduService
import android.os.Bundle

/**
 * The card half of a pairing tap: this phone answers a reader with the code it
 * is currently showing.
 *
 * Constructed by the system when a reader selects our AID, so it has no way to
 * be handed anything — the payload comes from [NfcAdapter.shared], which the
 * Dart side sets when a code goes on screen and clears when it comes off.
 *
 * That indirection is also the security property, such as it is. This service
 * exists whenever the app is installed and answers whenever the phone is
 * unlocked and touched to a reader; what stops it handing out an invite is that
 * there is no invite to hand out unless a person has asked for one. A code is
 * only in [NfcAdapter.shared] while its owner is holding the phone out.
 *
 * What a tap discloses, stated plainly: the same thing the QR code does — a
 * Layer-2 key and the rung id this device advertises under, linkable until the
 * next rotation. A tap is *narrower* than a screen, since it needs contact
 * rather than a line of sight, which is the one respect in which NFC is the
 * better half of R0-F4 rather than merely the more pleasant one.
 */
class HopplerApduService : HostApduService() {

    /** How far through [payload] the current exchange has got. */
    private var offset = 0
    private var payload: ByteArray? = null

    override fun processCommandApdu(command: ByteArray?, extras: Bundle?): ByteArray {
        val apdu = command ?: return NfcApdu.UNKNOWN

        if (NfcApdu.isSelect(apdu)) {
            // A fresh selection starts a fresh exchange. Taking the payload
            // once, here, rather than re-reading it per response: a code that
            // changed mid-tap would otherwise be served as two halves of two
            // different invites, which decodes to nothing and looks like a
            // broken tap rather than a race.
            val current = NfcAdapter.shared.get()
            if (current == null || current.isEmpty()) {
                payload = null
                return NfcApdu.NOTHING_TO_SHARE
            }
            payload = current
            offset = 0
            val response = NfcApdu.respond(current, 0)
            offset += NfcApdu.body(response.bytes).size
            return response.bytes
        }

        if (NfcApdu.isGetResponse(apdu)) {
            val current = payload ?: return NfcApdu.NOT_FOUND
            val response = NfcApdu.respond(current, offset, NfcApdu.requestedLength(apdu))
            offset += NfcApdu.body(response.bytes).size
            return response.bytes
        }

        return NfcApdu.UNKNOWN
    }

    /**
     * The phones came apart.
     *
     * The half-sent payload is dropped rather than kept for a resume: a reader
     * that comes back sends SELECT again and starts over, and holding a stale
     * offset would splice the tail of one tap onto the head of the next.
     */
    override fun onDeactivated(reason: Int) {
        payload = null
        offset = 0
    }
}

package org.hoppler.hoppler.nfc

import android.app.Activity
import android.nfc.NfcAdapter as PlatformNfc
import android.nfc.Tag
import android.nfc.tech.IsoDep
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.atomic.AtomicReference

/**
 * The NFC half of the pairing ceremony (T11, R0-F4) — the part that needs
 * hardware.
 *
 * The protocol lives in [NfcApdu] and is tested on the JVM; this is the
 * plumbing around it. Two roles, matching the QR path exactly:
 *
 * - **Sharing** — this phone is showing a code, and answers a reader as a card
 *   would. That is [HopplerApduService]; the payload it serves is set here,
 *   because the system constructs the service and we never hold an instance.
 * - **Reading** — this phone is looking for a code. Reader mode, `IsoDep`,
 *   SELECT, and as many GET RESPONSEs as the payload needs.
 *
 * A phone can do both at once as far as this class is concerned; the radio
 * decides otherwise while a reader field is up, which is why the UI never turns
 * both on.
 */
class NfcAdapter(private val activity: Activity) :
    MethodChannel.MethodCallHandler, EventChannel.StreamHandler {

    companion object {
        const val METHOD_CHANNEL = "org.hoppler/nfc"
        const val EVENT_CHANNEL = "org.hoppler/nfc/events"
        private const val TAG = "HopplerNfc"

        /** Event names, declared once. The Dart half asserts it knows the same
         * set — see `test/nfc_channel_vocabulary_test.dart`, which reads this
         * file, the same arrangement the BLE channel uses. */
        const val EVENT_CODE_READ = "codeRead"
        const val EVENT_UNREADABLE = "unreadable"

        /**
         * What the card serves, or null when this phone is not showing a code.
         *
         * Static because [HopplerApduService] is instantiated by the system on
         * a tap, with no way to hand it a reference. An `AtomicReference` rather
         * than a plain field because the service runs on its own binder thread
         * and the value is set from the UI one.
         */
        val shared = AtomicReference<ByteArray?>(null)
    }

    private var events: EventChannel.EventSink? = null
    private val main = Handler(Looper.getMainLooper())
    private var reading = false

    override fun onListen(arguments: Any?, sink: EventChannel.EventSink?) {
        events = sink
    }

    override fun onCancel(arguments: Any?) {
        events = null
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            // Whether a tap is possible at all. Answered honestly rather than
            // optimistically: plenty of phones have no NFC, and a "tap to pair"
            // affordance on one of them is an instruction nobody can follow.
            "isAvailable" -> result.success(available())
            "startSharing" -> {
                val code = call.argument<String>("code")
                if (code.isNullOrEmpty()) {
                    result.error("bad_args", "startSharing needs a code", null)
                    return
                }
                shared.set(code.toByteArray(Charsets.UTF_8))
                result.success(null)
            }
            "stopSharing" -> {
                shared.set(null)
                result.success(null)
            }
            "startReading" -> {
                if (!available()) {
                    result.error("unavailable", "this device has no usable NFC", null)
                    return
                }
                startReading()
                result.success(null)
            }
            "stopReading" -> {
                stopReading()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun available(): Boolean =
        PlatformNfc.getDefaultAdapter(activity)?.isEnabled == true

    private fun startReading() {
        if (reading) return
        val nfc = PlatformNfc.getDefaultAdapter(activity) ?: return
        // Platform sounds off: the tap is not the end of anything, and a
        // confirmation chime before two people have compared colours would be
        // the phone congratulating them on a ceremony that has not happened.
        val flags = PlatformNfc.FLAG_READER_NFC_A or
            PlatformNfc.FLAG_READER_NFC_B or
            PlatformNfc.FLAG_READER_NO_PLATFORM_SOUNDS or
            PlatformNfc.FLAG_READER_SKIP_NDEF_CHECK
        nfc.enableReaderMode(activity, { tag -> onTag(tag) }, flags, Bundle())
        reading = true
    }

    private fun stopReading() {
        if (!reading) return
        PlatformNfc.getDefaultAdapter(activity)?.disableReaderMode(activity)
        reading = false
    }

    /**
     * Read a code off a tag, on the callback's own thread.
     *
     * Everything here is best effort and nothing is retried: a tap that fails
     * is a tap, and the answer to one is to tap again. Reporting the failure
     * matters more than recovering from it — a person holding two phones
     * together needs to know it did not take.
     */
    private fun onTag(tag: Tag) {
        val iso = IsoDep.get(tag)
        if (iso == null) {
            report(EVENT_UNREADABLE, "that tap was not a Hoppler code")
            return
        }
        try {
            iso.connect()
            val first = iso.transceive(NfcApdu.selectCommand())
            if (!NfcApdu.isOk(first) && !NfcApdu.hasMore(first)) {
                // The other phone is not showing a code, or is not Hoppler.
                // One sentence for both: the person's next move is the same.
                report(EVENT_UNREADABLE, "no code to read on that phone")
                return
            }
            val gathered = NfcApdu.body(first).toMutableList()
            var response = first
            while (NfcApdu.hasMore(response)) {
                response = iso.transceive(NfcApdu.getResponseCommand(NfcApdu.MAX_BODY))
                gathered.addAll(NfcApdu.body(response).toList())
                if (!NfcApdu.isOk(response) && !NfcApdu.hasMore(response)) {
                    report(EVENT_UNREADABLE, "that tap was cut short")
                    return
                }
            }
            val code = String(gathered.toByteArray(), Charsets.UTF_8)
            if (code.isEmpty()) {
                report(EVENT_UNREADABLE, "no code to read on that phone")
                return
            }
            report(EVENT_CODE_READ, code)
        } catch (e: Exception) {
            // Moving a phone away mid-exchange lands here, and it is the normal
            // way a tap fails rather than an error worth dwelling on.
            Log.i(TAG, "tap did not complete: ${e.message}")
            report(EVENT_UNREADABLE, "that tap did not complete — try again")
        } finally {
            runCatching { iso.close() }
        }
    }

    private fun report(type: String, value: String) {
        main.post {
            events?.success(mapOf("type" to type, "value" to value))
        }
    }

    fun dispose() {
        stopReading()
        shared.set(null)
    }
}

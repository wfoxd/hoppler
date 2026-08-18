package org.hoppler.hoppler

import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodChannel
import org.hoppler.hoppler.ble.BleAdapter
import org.hoppler.hoppler.ble.BlePermissions
import org.hoppler.hoppler.nfc.NfcAdapter

class MainActivity : FlutterActivity() {
    private companion object {
        const val BLE_PERMISSION_REQUEST = 1
        const val PERMISSION_CHANNEL = "org.hoppler/permissions"

        /**
         * Process-scoped, not per-activity: asking again because the activity
         * was rebuilt is still asking again.
         */
        var asked = false
    }

    private var ble: BleAdapter? = null
    private var nfc: NfcAdapter? = null

    /**
     * The Dart calls waiting on a permission dialog, if one is up.
     *
     * `requestPermissions` answers through [onRequestPermissionsResult], so a
     * result has to be parked rather than returned. A list because a second
     * caller can arrive while the first prompt is still on screen, and the
     * right answer for it is the one the person is in the middle of giving —
     * not "denied", which would be answering on their behalf while they are
     * still looking at the dialog.
     *
     * Non-empty *is* "a prompt is in flight", which is what separates that case
     * from [asked], meaning one was shown and refused earlier.
     */
    private val awaitingRadio = mutableListOf<MethodChannel.Result>()

    /**
     * Held while the app is in the foreground so mDNS can be *received*.
     *
     * Android's Wi-Fi hardware drops incoming multicast unless something holds
     * this lock. Sending is unaffected, so a device without it advertises
     * normally and never hears a reply — one-way discovery, with no error
     * anywhere to explain it. Found on the first two-phone run: a Pixel 8 Pro
     * saw a Galaxy S20 FE and the S20 FE saw nothing.
     *
     * Tied to foreground because it costs battery and Ring 0 promises no
     * background delivery (R0-N6) — discovery is a foreground activity.
     */
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val adapter = BleAdapter(applicationContext)
        ble = adapter
        val messenger = flutterEngine.dartExecutor.binaryMessenger
        MethodChannel(messenger, BleAdapter.METHOD_CHANNEL).setMethodCallHandler(adapter)
        EventChannel(messenger, BleAdapter.EVENT_CHANNEL).setStreamHandler(adapter)

        // The activity, not the application context: reader mode is bound to a
        // foreground activity by the platform, which is also the honest shape
        // for R0-N6 — a tap is something a person is doing, not something the
        // app does while nobody is looking.
        val tap = NfcAdapter(this)
        nfc = tap
        MethodChannel(messenger, NfcAdapter.METHOD_CHANNEL).setMethodCallHandler(tap)
        EventChannel(messenger, NfcAdapter.EVENT_CHANNEL).setStreamHandler(tap)

        // Its own channel rather than a method on the BLE one: asking needs an
        // *Activity*, and `BleAdapter` deliberately holds the application
        // context so it can outlive one. Putting the ask there would mean
        // handing it an activity reference for this alone.
        MethodChannel(messenger, PERMISSION_CHANNEL).setMethodCallHandler { call, result ->
            when (call.method) {
                "ensureRadio" -> ensureRadio(result)
                else -> result.notImplemented()
            }
        }
    }

    /**
     * Ask for the Bluetooth permissions, now that somebody has asked to be
     * discoverable.
     *
     * Called from the Discovery toggle rather than from `onResume`, which is
     * where it used to live. On first launch that put "allow Hoppler to find,
     * connect to and determine the relative position of nearby devices" in
     * front of a person who had not yet said who they were, let alone that they
     * wanted to be found — the prompt most likely to be denied, at the moment
     * it makes least sense. Android's own guidance is to ask in context, and
     * Hoppler has one: this switch.
     *
     * Answers `true` when the permissions are held. A refusal answers `false`
     * rather than an error: being denied is a thing a person chose, not a fault.
     */
    private fun ensureRadio(result: MethodChannel.Result) {
        val state = BlePermissions.state {
            checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
        }
        if (state is BlePermissions.State.Granted) {
            result.success(true)
            return
        }
        // A prompt is already up. Join it: the person is mid-decision, and
        // answering "denied" for them because they were slow would be a switch
        // that refused itself while they were reaching for Allow.
        if (awaitingRadio.isNotEmpty()) {
            awaitingRadio += result
            return
        }
        // A second ask *after* a refusal shows no dialog at all — Android
        // answers "denied" immediately — so it would be a switch that does
        // nothing with no way for the person to tell why. Answered honestly
        // instead; the route back is Settings, and `onResume` notices a grant
        // made there.
        if (asked) {
            result.success(false)
            return
        }
        asked = true
        awaitingRadio += result
        requestPermissions(
            (state as BlePermissions.State.Missing).permissions.toTypedArray(),
            BLE_PERMISSION_REQUEST,
        )
    }

    override fun onResume() {
        super.onResume()
        if (multicastLock == null) {
            // Safe cast: the service can be absent on stripped or unusual
            // builds, and a hard cast would take the activity down on resume —
            // trading "discovery is degraded" for "the app will not open".
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
                ?: return
            multicastLock = wifi.createMulticastLock("hoppler-mdns").apply {
                setReferenceCounted(false)
                acquire()
            }
        }
        // A grant made in Settings while we were away is invisible to the
        // adapter — Android broadcasts nothing for it, and the app is restarted
        // only if a permission is *revoked*. Resume is where it gets noticed,
        // and the re-arm on the way back through is what actually starts the
        // radio. This half stays; what used to sit beside it — the ask itself —
        // moved to [ensureRadio], because a resume is not a moment anybody
        // asked to be found.
        ble?.refreshAvailability()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != BLE_PERMISSION_REQUEST) return
        // Reported either way, and a grant is the case that matters: it is what
        // flips the rung to available, which is what re-arms the radio. Without
        // this the permission is held and nothing has started using it.
        ble?.refreshAvailability()
        // Answer the Dart call that is still waiting on this dialog. Both
        // outcomes answer it: a switch left spinning because somebody declined
        // is worse than one that goes back to off.
        val granted = grantResults.isNotEmpty() &&
            grantResults.all { it == PackageManager.PERMISSION_GRANTED }
        awaitingRadio.forEach { it.success(granted) }
        awaitingRadio.clear()
    }

    override fun onPause() {
        // Released rather than held for the process lifetime: an app that is
        // not on screen is not discovering, and the radio cost is real.
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
        // Reader mode is bound to a foreground activity, and the platform is
        // entitled to complain if it outlives one. Also the honest behaviour:
        // a tap is something a person is doing, so an app in the background has
        // no business holding the field open. The *card* half stays — it is the
        // system that answers a reader, and it only has something to answer
        // with while a code is on screen.
        nfc?.dispose()
        super.onPause()
    }

    override fun onDestroy() {
        // Anything still waiting on a dialog is answered before the activity
        // goes. A `Result` that is never completed is a Dart `await` that never
        // returns, which here is a Discovery switch stuck mid-flip for the rest
        // of the session.
        awaitingRadio.forEach { it.success(false) }
        awaitingRadio.clear()
        // Radios outlive activities unless something stops them; leaving one
        // advertising after the app is gone is the R0-F2 failure the whole
        // rung exists to avoid.
        ble?.shutdown()
        ble = null
        nfc?.dispose()
        nfc = null
        super.onDestroy()
    }
}

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

class MainActivity : FlutterActivity() {
    private companion object {
        const val BLE_PERMISSION_REQUEST = 1

        /**
         * Process-scoped, not per-activity: asking again because the activity
         * was rebuilt is still asking again.
         */
        var asked = false
    }

    private var ble: BleAdapter? = null

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
        askForTheRadio()
    }

    /**
     * Ask for the Bluetooth permissions, once.
     *
     * Nothing asked before this. BLE worked on the two phones it was developed
     * on because their permissions had been granted by hand over `adb`, and
     * would have failed on every other device as an empty peer list — the app
     * has no way to reach the radio and no way to ask for one.
     *
     * Once per process rather than once per resume: after a second refusal
     * Android answers immediately with "denied" and shows no dialog, so asking
     * on every resume would be a loop the user cannot get out of. A refusal is
     * answered by an availability event carrying the reason, and a grant made
     * later in Settings is picked up by the re-check below.
     *
     * A refusal reaches the screen as the reason on an availability event, so
     * an empty list says why it is empty rather than reading as "nobody is
     * nearby". On Android 11 and older the permission asked for is location,
     * and the reason explains that too — see `BlePermissions.reason`.
     */
    private fun askForTheRadio() {
        val state = BlePermissions.state(
            locationServicesOn = BlePermissions.locationServicesOn(this),
        ) {
            checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
        }
        if (state is BlePermissions.State.Missing && !asked) {
            asked = true
            requestPermissions(state.permissions.toTypedArray(), BLE_PERMISSION_REQUEST)
            return
        }
        // A grant made in Settings while we were away is invisible to the
        // adapter — Android broadcasts nothing for it, and the app is
        // restarted only if a permission is *revoked*. Resume is where it gets
        // noticed, and the re-arm on the way back through is what actually
        // starts the radio.
        ble?.refreshAvailability()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        // Reported either way, and a grant is the case that matters: it is what
        // flips the rung to available, which is what re-arms the radio. Without
        // this the permission is held and nothing has started using it.
        if (requestCode == BLE_PERMISSION_REQUEST) ble?.refreshAvailability()
    }

    override fun onPause() {
        // Released rather than held for the process lifetime: an app that is
        // not on screen is not discovering, and the radio cost is real.
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
        super.onPause()
    }

    override fun onDestroy() {
        // Radios outlive activities unless something stops them; leaving one
        // advertising after the app is gone is the R0-F2 failure the whole
        // rung exists to avoid.
        ble?.shutdown()
        ble = null
        super.onDestroy()
    }
}

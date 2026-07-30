package org.hoppler.hoppler

import android.content.Context
import android.net.wifi.WifiManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodChannel
import org.hoppler.hoppler.ble.BleAdapter

class MainActivity : FlutterActivity() {
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
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            multicastLock = wifi.createMulticastLock("hoppler-mdns").apply {
                setReferenceCounted(false)
                acquire()
            }
        }
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

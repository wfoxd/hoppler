package org.hoppler.hoppler

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
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

    /**
     * Watches for the interface being replaced, so the lock above can be
     * re-taken against the new one.
     *
     * A `MulticastLock` is taken against the Wi-Fi interface that exists when
     * it is acquired. Aeroplane mode — or any reconnect — tears that interface
     * down and builds a new one, leaving the held lock referring to something
     * gone and multicast filtering silently back on. The symptom is not an
     * error: discovery simply goes quiet ("No one nearby") while advertising
     * carries on working, because only the receive path is filtered.
     *
     * Found on hardware after the lock itself was added — the fix for one bug
     * having its own.
     */
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

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
        acquireMulticastLock()
        watchForNetworkChanges()
    }

    override fun onPause() {
        // Released rather than held for the process lifetime: an app that is
        // not on screen is not discovering, and the radio cost is real.
        stopWatchingForNetworkChanges()
        releaseMulticastLock()
        super.onPause()
    }

    /** Take a fresh lock, dropping any held against a previous interface. */
    private fun acquireMulticastLock() {
        releaseMulticastLock()
        // Safe cast: the service can be absent on stripped or unusual builds,
        // and a hard cast would take the activity down on resume — trading
        // "discovery is degraded" for "the app will not open".
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            ?: return
        multicastLock = wifi.createMulticastLock("hoppler-mdns").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseMulticastLock() {
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
    }

    private fun watchForNetworkChanges() {
        if (networkCallback != null) return
        val cm = applicationContext.getSystemService(Context.CONNECTIVITY_SERVICE)
            as? ConnectivityManager ?: return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                // A new interface: whatever we hold is against the old one.
                runOnUiThread { acquireMulticastLock() }
            }
        }
        networkCallback = callback
        runCatching { cm.registerDefaultNetworkCallback(callback) }
            .onFailure { networkCallback = null }
    }

    private fun stopWatchingForNetworkChanges() {
        val cm = applicationContext.getSystemService(Context.CONNECTIVITY_SERVICE)
            as? ConnectivityManager
        networkCallback?.let { cb -> runCatching { cm?.unregisterNetworkCallback(cb) } }
        networkCallback = null
    }

    override fun onDestroy() {
        // Radios outlive activities unless something stops them; leaving one
        // advertising after the app is gone is the R0-F2 failure the whole
        // rung exists to avoid.
        ble?.shutdown()
        ble = null
        stopWatchingForNetworkChanges()
        releaseMulticastLock()
        super.onDestroy()
    }
}

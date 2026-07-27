package org.hoppler.hoppler

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodChannel
import org.hoppler.hoppler.ble.BleAdapter

class MainActivity : FlutterActivity() {
    private var ble: BleAdapter? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val adapter = BleAdapter(applicationContext)
        ble = adapter
        val messenger = flutterEngine.dartExecutor.binaryMessenger
        MethodChannel(messenger, BleAdapter.METHOD_CHANNEL).setMethodCallHandler(adapter)
        EventChannel(messenger, BleAdapter.EVENT_CHANNEL).setStreamHandler(adapter)
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

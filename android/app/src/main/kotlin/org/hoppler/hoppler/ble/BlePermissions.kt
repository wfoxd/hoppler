package org.hoppler.hoppler.ble

import android.Manifest
import android.os.Build

/**
 * Which permissions the radio needs, and whether this device has granted them.
 *
 * Split out of [BleAdapter] because the answer is pure logic over an SDK level
 * and a set of grants — no radio, no Android runtime, so it runs in CI — and
 * because the defect it exists to prevent is the one this rung is most exposed
 * to: BLE worked on the two phones it was developed on **only** because their
 * permissions had been granted by hand over `adb`. On any other device the
 * scan throws, the peer list stays empty, and the app says "nobody is nearby".
 */
object BlePermissions {

    /**
     * Android 12 replaced the location-shaped Bluetooth permissions with three
     * that say what they mean. Hoppler declares exactly those, with
     * `neverForLocation` on the scan, because it has no use for location and
     * asking for it would contradict G-2.
     */
    val RUNTIME: List<String> = listOf(
        Manifest.permission.BLUETOOTH_ADVERTISE,
        Manifest.permission.BLUETOOTH_SCAN,
        Manifest.permission.BLUETOOTH_CONNECT,
    )

    sealed interface State {
        /** Everything the radio needs is held. */
        data object Granted : State

        /** These are not granted. Asking is the only way forward. */
        data class Missing(val permissions: List<String>) : State

        /** This Android cannot run the rung at all — see [reason]. */
        data class Unsupported(val reason: String) : State
    }

    /**
     * The live permission state.
     *
     * `sdkInt` and `granted` are parameters rather than reads of the ambient
     * runtime so the branch below can be tested for the versions we do not
     * develop on, which is where it matters.
     */
    fun state(
        sdkInt: Int = Build.VERSION.SDK_INT,
        granted: (String) -> Boolean,
    ): State {
        // Below API 31 a BLE scan required ACCESS_FINE_LOCATION — Android
        // treated "what is near me" as a location fix until 12. Hoppler
        // declines location outright (G-2), so on Android 10 and 11 the rung is
        // honestly unavailable rather than quietly finding nobody. minSdk is
        // 29, so this is a real device and not a hypothetical one.
        if (sdkInt < Build.VERSION_CODES.S) {
            return State.Unsupported("Bluetooth needs Android 12 or later")
        }
        val missing = RUNTIME.filterNot(granted)
        return if (missing.isEmpty()) State.Granted else State.Missing(missing)
    }

    /**
     * Why the radio cannot be used, or `null` if it can — phrased for the
     * person holding the phone, because this reaches the UI as the reason on an
     * availability event.
     */
    fun reason(state: State): String? = when (state) {
        is State.Granted -> null
        is State.Missing -> "Hoppler needs permission to use Bluetooth"
        is State.Unsupported -> state.reason
    }
}

package org.hoppler.hoppler.ble

import android.Manifest

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
     * `neverForLocation` on the scan, because it has no use for location.
     *
     * Not, as this said for months, because location "would contradict G-2".
     * G-2 is about server-side identity and on-device metadata; a permission
     * prompt sends nothing anywhere and stores nothing. The objection is that
     * asking for a capability the app has no use for costs a privacy tool
     * trust — a good reason, and a product one. Dressing it as a requirement
     * made a decision look already-made for months.
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
    }

    /**
     * The live permission state.
     *
     * No version branch: `minSdk` is 31, so [RUNTIME] is what every supported
     * device uses. Below that a scan needed `ACCESS_FINE_LOCATION`, which
     * Hoppler does not ask for, and the floor was raised rather than ask —
     * T08b §5.0.23 for the costing.
     *
     * The `Unsupported` state and the `sdkInt` parameter went with that branch.
     * Keeping a case no supported device can reach would be a guard that cannot
     * fire pretending to protect something, and this project has now found that
     * shape in production three times; there is no sense adding a fourth on
     * purpose.
     */
    fun state(granted: (String) -> Boolean): State {
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
    }
}

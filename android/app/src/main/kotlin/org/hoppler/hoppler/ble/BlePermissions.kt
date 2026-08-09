package org.hoppler.hoppler.ble

import android.Manifest
import android.content.Context
import android.location.LocationManager
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
     * that say what they mean. On 12 and later Hoppler declares exactly those,
     * with `neverForLocation` on the scan, because it has no use for location.
     */
    val RUNTIME: List<String> = listOf(
        Manifest.permission.BLUETOOTH_ADVERTISE,
        Manifest.permission.BLUETOOTH_SCAN,
        Manifest.permission.BLUETOOTH_CONNECT,
    )

    /**
     * Android 11 and older, which `minSdk = 29` means are real devices.
     *
     * The three above do not exist there, and there is no `neverForLocation` to
     * decline with: until Android 12 the platform treated "what is near me" as
     * a location fix, so a BLE scan requires `ACCESS_FINE_LOCATION` outright.
     * Without it the scan does not throw — it returns nothing, forever, which
     * is the silent empty list this object exists to prevent.
     *
     * Declared with `android:maxSdkVersion="30"` in the manifest, so nothing
     * here is requested on Android 12 and later.
     */
    val LEGACY_RUNTIME: List<String> = listOf(Manifest.permission.ACCESS_FINE_LOCATION)

    /** What this Android needs granted before the rung can run. */
    fun required(sdkInt: Int): List<String> =
        if (sdkInt >= Build.VERSION_CODES.S) RUNTIME else LEGACY_RUNTIME

    sealed interface State {
        /** Everything the radio needs is held. */
        data object Granted : State

        /** These are not granted. Asking is the only way forward. */
        data class Missing(val permissions: List<String>) : State

        /**
         * Granted, and still unable to scan — see [reason].
         *
         * A separate state because the fix is not a permission dialog. Asking
         * again would show nothing and change nothing, so a caller that treats
         * every failure as "request the permissions" would loop in silence.
         */
        data class Blocked(val reason: String) : State
    }

    /**
     * The live permission state.
     *
     * `sdkInt`, `locationServicesOn` and `granted` are parameters rather than
     * reads of the ambient runtime so the branches below can be tested for the
     * versions and settings we do not develop on, which is where it matters.
     */
    fun state(
        sdkInt: Int = Build.VERSION.SDK_INT,
        locationServicesOn: Boolean = true,
        granted: (String) -> Boolean,
    ): State {
        val missing = required(sdkInt).filterNot(granted)
        if (missing.isNotEmpty()) return State.Missing(missing)

        // Below Android 12 the permission is necessary and *not sufficient*:
        // the system location toggle has to be on as well, or scan results are
        // withheld with no error and no callback. Holding the permission and
        // finding nobody is exactly the failure this file exists to stop, and
        // it is the one thing option 3 could have shipped as a silent hole.
        if (sdkInt < Build.VERSION_CODES.S && !locationServicesOn) {
            return State.Blocked(
                "Turn on Location. Android 11 and older report no Bluetooth " +
                    "devices at all while it is off."
            )
        }
        return State.Granted
    }

    /**
     * Whether the system location toggle is on.
     *
     * The impure edge, kept out of [state] so that stays testable. Reads a
     * setting, not a position: no location is requested and none is received.
     */
    fun locationServicesOn(context: Context): Boolean =
        (context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager)
            ?.isLocationEnabled == true

    /**
     * Why the radio cannot be used, or `null` if it can — phrased for the
     * person holding the phone, because this reaches the UI as the reason on an
     * availability event.
     *
     * The older-Android wording says what is being asked for and why, and that
     * the answer is not used for anything. A privacy tool that asks for precise
     * location without explaining itself has told the user something untrue
     * about what it is, whatever the manifest says.
     */
    fun reason(state: State): String? = when (state) {
        is State.Granted -> null
        is State.Blocked -> state.reason
        is State.Missing ->
            if (Manifest.permission.ACCESS_FINE_LOCATION in state.permissions) {
                "Hoppler needs location permission, because this version of " +
                    "Android counts every Bluetooth scan as one. Your location " +
                    "is not read, stored, or sent anywhere."
            } else {
                "Hoppler needs permission to use Bluetooth"
            }
    }
}

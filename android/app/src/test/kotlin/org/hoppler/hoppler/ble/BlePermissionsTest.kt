package org.hoppler.hoppler.ble

import android.Manifest
import android.os.Build
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The permission gate, checked for the Android versions and grant combinations
 * two development phones will never produce.
 *
 * It exists because the rung's worst failure is the invisible one: BLE worked
 * on the two phones it was built on **only** because their permissions had been
 * granted by hand over `adb`. Anywhere else the scan throws, the list stays
 * empty, and the app reads as "nobody is nearby".
 */
class BlePermissionsTest {

    private fun state(sdk: Int, held: Set<String>, locationOn: Boolean = true) =
        BlePermissions.state(sdk, locationOn) { it in held }

    private val fineLocation = Manifest.permission.ACCESS_FINE_LOCATION

    @Test
    fun `all three granted is granted`() {
        assertEquals(
            BlePermissions.State.Granted,
            state(Build.VERSION_CODES.S, BlePermissions.RUNTIME.toSet())
        )
    }

    /**
     * One missing is as fatal as three, and each fails differently enough to be
     * worth stating: no advertise is a device nobody can see, no scan is a
     * device that can see nobody, no connect is a pair that find each other and
     * cannot speak. All three present as the other phone being broken.
     */
    @Test
    fun `any single permission missing is missing`() {
        BlePermissions.RUNTIME.forEach { withheld ->
            assertEquals(
                "withholding $withheld",
                BlePermissions.State.Missing(listOf(withheld)),
                state(Build.VERSION_CODES.S, BlePermissions.RUNTIME.toSet() - withheld)
            )
        }
    }

    /** The fresh-install case: ask for everything, in one dialog. */
    @Test
    fun `nothing granted asks for all of them`() {
        assertEquals(
            BlePermissions.State.Missing(BlePermissions.RUNTIME),
            state(Build.VERSION_CODES.S, emptySet())
        )
    }

    // ── Android 10 and 11 ───────────────────────────────────────────────────

    /**
     * minSdk is 29, so these are supported devices, and the modern three do not
     * exist on them. They are granted in these tests on purpose: a gate that
     * only counted grants would call such a device ready and then scan forever.
     */
    @Test
    fun `below Android 12 the modern three count for nothing`() {
        for (sdk in 29..30) {
            assertEquals(
                "API $sdk",
                BlePermissions.State.Missing(listOf(fineLocation)),
                state(sdk, BlePermissions.RUNTIME.toSet())
            )
        }
    }

    @Test
    fun `below Android 12 location permission is what makes the rung usable`() {
        for (sdk in 29..30) {
            assertEquals(
                "API $sdk",
                BlePermissions.State.Granted,
                state(sdk, setOf(fineLocation))
            )
        }
    }

    /**
     * The hole option 3 could have shipped.
     *
     * On Android 11 and older the permission is necessary and *not sufficient*:
     * with the system location toggle off, scan results are withheld with no
     * error and no callback. Granted-but-finding-nobody is precisely the silent
     * failure this file exists to prevent, so it has to be a state of its own.
     */
    @Test
    fun `below Android 12 location switched off blocks the rung even when granted`() {
        for (sdk in 29..30) {
            val s = state(sdk, setOf(fineLocation), locationOn = false)
            assertTrue("API $sdk", s is BlePermissions.State.Blocked)
            assertNotNull("API $sdk needs something to show", BlePermissions.reason(s))
        }
    }

    /** …and on Android 12+ the toggle is irrelevant, because the scan is not a location fix. */
    @Test
    fun `from Android 12 the location toggle does not matter`() {
        assertEquals(
            BlePermissions.State.Granted,
            state(Build.VERSION_CODES.S, BlePermissions.RUNTIME.toSet(), locationOn = false)
        )
    }

    /**
     * Asking for precise location without saying why is the thing a privacy
     * tool cannot do, so the older-Android wording must not be the generic one.
     */
    @Test
    fun `asking for location explains itself`() {
        val legacy = BlePermissions.reason(BlePermissions.State.Missing(listOf(fineLocation)))
        val modern = BlePermissions.reason(BlePermissions.State.Missing(BlePermissions.RUNTIME))
        assertNotNull(legacy)
        assertNotEquals("the location prompt got the generic Bluetooth wording", modern, legacy)
        assertTrue(
            "the reason must say the word, or it explains nothing: $legacy",
            legacy!!.contains("location", ignoreCase = true)
        )
    }

    /**
     * Exactly one state has no reason. The reason is what the UI puts in place
     * of an empty list, so a `null` from an unusable state is the silent
     * failure coming straight back.
     */
    @Test
    fun `only a usable radio has no reason to give`() {
        assertNull(BlePermissions.reason(BlePermissions.State.Granted))
        assertNotNull(BlePermissions.reason(BlePermissions.State.Missing(BlePermissions.RUNTIME)))
        assertNotNull(BlePermissions.reason(BlePermissions.State.Blocked("location is off")))
    }
}

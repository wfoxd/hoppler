package org.hoppler.hoppler.ble

import android.os.Build
import org.junit.Assert.assertEquals
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

    private fun state(sdk: Int, held: Set<String>) =
        BlePermissions.state(sdk) { it in held }

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

    /**
     * minSdk is 29, so Android 10 and 11 are supported devices — and there a
     * BLE scan needs `ACCESS_FINE_LOCATION`, which Hoppler does not ask for and
     * does not declare. The rung cannot work on them, so it must say so rather
     * than scan forever and find nobody.
     *
     * The three runtime permissions are granted in this test on purpose: they
     * do not exist below API 31, so a gate that only counted grants would call
     * this device ready.
     */
    @Test
    fun `below Android 12 the rung is unsupported however much is granted`() {
        for (sdk in 29..30) {
            val state = state(sdk, BlePermissions.RUNTIME.toSet())
            assertTrue("API $sdk", state is BlePermissions.State.Unsupported)
            assertNotNull("API $sdk needs something to show", BlePermissions.reason(state))
        }
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
        assertNotNull(BlePermissions.reason(BlePermissions.State.Unsupported("no radio")))
    }
}

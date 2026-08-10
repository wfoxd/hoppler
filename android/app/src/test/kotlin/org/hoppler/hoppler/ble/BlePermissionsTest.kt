package org.hoppler.hoppler.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The permission gate, checked for the grant combinations two development
 * phones will never produce.
 *
 * It exists because the rung's worst failure is the invisible one: BLE worked
 * on the two phones it was built on **only** because their permissions had been
 * granted by hand over `adb`. Anywhere else the scan throws, the list stays
 * empty, and the app reads as "nobody is nearby".
 *
 * The version cases are gone with the branch they covered. `minSdk` is 31, so
 * every supported device needs the same three permissions — T08b §5.0.23 for
 * why the floor moved rather than Hoppler asking for location.
 */
class BlePermissionsTest {

    private fun state(held: Set<String>) = BlePermissions.state { it in held }

    @Test
    fun `all three granted is granted`() {
        assertEquals(BlePermissions.State.Granted, state(BlePermissions.RUNTIME.toSet()))
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
                state(BlePermissions.RUNTIME.toSet() - withheld)
            )
        }
    }

    /** The fresh-install case: ask for everything, in one dialog. */
    @Test
    fun `nothing granted asks for all of them`() {
        assertEquals(BlePermissions.State.Missing(BlePermissions.RUNTIME), state(emptySet()))
    }

    /**
     * Hoppler asks for no location permission of any kind, on any supported
     * version. Raising the floor to 31 is what bought that, so it is worth
     * asserting rather than trusting a comment: an addition to [RUNTIME] that
     * reached for location would otherwise pass every test above and give the
     * decision away for nothing.
     */
    @Test
    fun `the radio never asks for location`() {
        BlePermissions.RUNTIME.forEach {
            assertEquals(
                "$it is a location permission, which is what raising minSdk avoided",
                false,
                it.contains("LOCATION", ignoreCase = true)
            )
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
    }
}

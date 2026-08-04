package org.hoppler.hoppler.ble

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The sentence shown when a dial cannot get a link.
 *
 * It exists because the failure it describes took eight two-phone runs to
 * explain, and every one of those runs the app said only that Ping had failed.
 * The count is the whole diagnosis (§5.0.15), so the message has to carry it —
 * and has to keep carrying it, which is what these assertions are for.
 */
class LinkFailureReasonTest {

    private fun reason(links: Int) = BleAdapter.linkFailureReason(links)

    @Test
    fun `a crowded phone is told it is crowded, and what to do`() {
        val said = reason(8)
        assertTrue("the count is the diagnosis and has to appear", said.contains("8"))
        assertTrue(
            "a person who cannot act on it is no better off than before",
            said.contains("off and on")
        )
    }

    @Test
    fun `the threshold is where the observed failure was total`() {
        assertTrue("seven is full", reason(7).contains("usually allows"))
        assertFalse("six is not", reason(6).contains("usually allows"))
    }

    @Test
    fun `the ceiling is offered as typical, never asserted as the limit`() {
        // The real maximum is not readable from an app and differs by device.
        // Stating it as fact would be false on any phone with a larger pool,
        // and at twenty open it would be absurd on its face — which is the
        // overclaim this whole rung keeps producing.
        for (links in listOf(7, 8, 20)) {
            val said = reason(links)
            assertTrue("the measured count is ours to assert", said.contains("$links"))
            assertTrue("the ceiling is hedged", said.contains("usually allows around"))
            assertFalse(
                "a device with a bigger pool would be told something false",
                said.contains("as many as Android allows")
            )
        }
    }

    @Test
    fun `an uncrowded phone is not blamed for being crowded`() {
        val said = reason(0)
        assertFalse(
            "claiming exhaustion at zero links would send the reader after the " +
                "wrong cause, which is exactly what this message exists to stop",
            said.contains("usually allows")
        )
        assertTrue(said.contains("0 already open"))
    }

    @Test
    fun `a count we do not have is not reported as a count we do`() {
        // -1 is the adapter failing to read the number at all. Rendering that
        // as "-1 already open" would be worse than saying nothing.
        val said = reason(-1)
        assertFalse(said.contains("-1"))
        assertFalse(said.contains("already open"))
        assertTrue(said.contains("Could not open a Bluetooth connection"))
    }

    @Test
    fun `no reason carries an address`() {
        // R0-F2 does not stop at the log: this string reaches the screen, and a
        // colon-separated hex pair is what a Bluetooth address looks like.
        val addressish = Regex("""\b[0-9A-Fa-f]{2}:[0-9A-Fa-f]{2}""")
        for (links in listOf(-1, 0, 6, 7, 20)) {
            assertFalse(
                "a reason must never carry an address",
                addressish.containsMatchIn(reason(links))
            )
        }
    }
}

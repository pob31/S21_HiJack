package com.pob31.s21monitor.net

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Audit A8: which network the link's socket is bound to. */
class NetworksTest {

    @Test
    fun theNetworkWhoseSubnetHoldsTheDaemonWins() {
        val options = listOf(
            NetworkOption("mobile", isWifi = false, reachesDirectly = false),
            NetworkOption("show wifi", isWifi = true, reachesDirectly = true),
        )
        assertEquals("show wifi", chooseNetwork(options))
    }

    @Test
    fun aWiredAdapterOnTheDesksSubnetBeatsWifi() {
        val options = listOf(
            NetworkOption("home wifi", isWifi = true, reachesDirectly = false),
            NetworkOption("usb ethernet", isWifi = false, reachesDirectly = true),
        )
        assertEquals("usb ethernet", chooseNetwork(options))
    }

    @Test
    fun otherwiseWifi() {
        // The daemon sits behind a router on the show network.
        val options = listOf(
            NetworkOption("mobile", isWifi = false, reachesDirectly = false),
            NetworkOption("show wifi", isWifi = true, reachesDirectly = false),
        )
        assertEquals("show wifi", chooseNetwork(options))
    }

    @Test
    fun noWifiAndNoRouteLeavesItToAndroid() {
        val options = listOf(NetworkOption("mobile", isWifi = false, reachesDirectly = false))
        assertNull(chooseNetwork(options))
    }
}

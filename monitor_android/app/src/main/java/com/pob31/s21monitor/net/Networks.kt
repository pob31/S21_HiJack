package com.pob31.s21monitor.net

import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import java.net.InetAddress

/** A network a socket could be bound to, as seen when choosing one. */
data class NetworkOption<N>(
    val network: N,
    val isWifi: Boolean,
    /** The target is on this network's own subnet (a non-default route). */
    val reachesDirectly: Boolean,
)

/**
 * The network to bind a socket to (audit A8): one whose subnet holds the
 * target, else Wi-Fi, else none (leave it to Android). On show Wi-Fi without
 * internet Android makes mobile data the default network, and an unbound
 * socket's traffic to the desk's LAN then goes out over mobile data.
 */
fun <N> chooseNetwork(options: List<NetworkOption<N>>): N? =
    (options.firstOrNull { it.reachesDirectly } ?: options.firstOrNull { it.isWifi })?.network

/** The networks up right now, rated for reaching [target] (null: no
 *  particular address, e.g. a broadcast). */
fun networkOptions(cm: ConnectivityManager, target: InetAddress?): List<NetworkOption<Network>> {
    // allNetworks is deprecated in favour of callbacks, but a one-off look
    // at what's up is all that's needed here.
    @Suppress("DEPRECATION")
    val networks = cm.allNetworks
    return networks.map { n ->
        val wifi = cm.getNetworkCapabilities(n)
            ?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true
        val direct = target != null && cm.getLinkProperties(n)?.routes.orEmpty()
            .any { !it.isDefaultRoute && it.matches(target) }
        NetworkOption(n, wifi, direct)
    }
}

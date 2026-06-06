package com.pob31.s21monitor.data

import android.content.Context
import com.pob31.s21monitor.model.Credentials

/**
 * Persists the connection profile so the foreground service can reload it on
 * (re)start — the service runs independently of the UI, so it reads creds
 * itself rather than receiving them across a binding. SharedPreferences (no
 * extra dependency), mirroring the WFS-DIY remote.
 */
object CredentialsStore {
    private const val PREFS = "s21_monitor_prefs"
    private const val KEY_NAME = "name"
    private const val KEY_HOST = "host"
    private const val KEY_PORT = "port"

    fun save(context: Context, creds: Credentials) {
        context.prefs().edit()
            .putString(KEY_NAME, creds.name)
            .putString(KEY_HOST, creds.host)
            .putInt(KEY_PORT, creds.port)
            .apply()
    }

    fun load(context: Context): Credentials? {
        val p = context.prefs()
        val name = p.getString(KEY_NAME, null) ?: return null
        val host = p.getString(KEY_HOST, null) ?: return null
        if (name.isBlank() || host.isBlank()) return null
        return Credentials(name, host, p.getInt(KEY_PORT, 8025))
    }

    fun clear(context: Context) {
        context.prefs().edit().clear().apply()
    }

    private fun Context.prefs() =
        getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}

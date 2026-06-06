package com.pob31.s21monitor.ui

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import com.pob31.s21monitor.data.CredentialsStore
import com.pob31.s21monitor.model.Credentials
import com.pob31.s21monitor.service.MonitorService
import com.pob31.s21monitor.ui.theme.Bg
import com.pob31.s21monitor.ui.theme.Muted
import com.pob31.s21monitor.ui.theme.S21MonitorTheme

class MainActivity : ComponentActivity() {

    private val service = mutableStateOf<MonitorService?>(null)
    private var bound = false

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
            service.value = (binder as? MonitorService.LocalBinder)?.service()
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            service.value = null
        }
    }

    private val notifPermLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { /* best effort */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        maybeRequestNotificationPermission()
        setContent {
            S21MonitorTheme { Root() }
        }
    }

    override fun onStart() {
        super.onStart()
        // Reattach to (or start) the service whenever a profile is configured.
        if (CredentialsStore.load(this) != null) startAndBind()
    }

    override fun onStop() {
        super.onStop()
        if (bound) {
            unbindService(connection)
            bound = false
            // The service stays running (started + foreground); we just detach.
        }
    }

    private fun startAndBind() {
        val intent = Intent(this, MonitorService::class.java)
        ContextCompat.startForegroundService(this, intent)
        if (!bound) {
            bindService(intent, connection, Context.BIND_AUTO_CREATE)
            bound = true
        }
    }

    private fun connectWith(creds: Credentials) {
        CredentialsStore.save(this, creds)
        startAndBind()
    }

    private fun forget() {
        service.value?.shutdown()
        if (bound) {
            unbindService(connection)
            bound = false
        }
        service.value = null
        CredentialsStore.clear(this)
    }

    private fun maybeRequestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val granted = ContextCompat.checkSelfPermission(
                this, Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
            if (!granted) notifPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    @Composable
    private fun Root() {
        var creds by remember { mutableStateOf(CredentialsStore.load(this)) }
        val svc = service.value
        val current = creds

        when {
            current == null -> ConnectionScreen(initial = null, onConnect = { c ->
                connectWith(c)
                creds = c
            })

            svc == null -> Loading()

            else -> MonitorRoute(svc, current.name, onShutdown = {
                forget()
                creds = null
            })
        }
    }

    @Composable
    private fun MonitorRoute(svc: MonitorService, clientName: String, onShutdown: () -> Unit) {
        val state by svc.state.collectAsState()
        MonitorScreen(
            state = state,
            clientName = clientName,
            service = svc,
            onShutdown = onShutdown,
        )
    }

    @Composable
    private fun Loading() {
        Box(Modifier.fillMaxSize().background(Bg), contentAlignment = Alignment.Center) {
            Text("Connecting…", color = Muted)
        }
    }
}

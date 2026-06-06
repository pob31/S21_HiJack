package com.pob31.s21monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.pob31.s21monitor.discovery.Discovery
import com.pob31.s21monitor.model.Credentials
import com.pob31.s21monitor.ui.theme.Bg
import com.pob31.s21monitor.ui.theme.Muted
import com.pob31.s21monitor.ui.theme.Panel
import com.pob31.s21monitor.ui.theme.TextPrimary
import kotlinx.coroutines.launch

@Composable
fun ConnectionScreen(
    initial: Credentials?,
    onConnect: (Credentials) -> Unit,
) {
    var name by remember { mutableStateOf(initial?.name ?: "") }
    var host by remember { mutableStateOf(initial?.host ?: "") }
    var port by remember { mutableStateOf((initial?.port ?: 8025).toString()) }
    var discovering by remember { mutableStateOf(false) }
    var results by remember { mutableStateOf<List<Discovery.Found>>(emptyList()) }
    val scope = rememberCoroutineScope()

    Column(
        Modifier.fillMaxSize().background(Bg).verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("S21 Monitor", color = TextPrimary, fontSize = 24.sp, fontWeight = FontWeight.Bold)
        Text("Connect to the daemon on your network.", color = Muted, fontSize = 14.sp)

        OutlinedTextField(
            value = name, onValueChange = { name = it },
            label = { Text("Profile name") },
            singleLine = true, modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = host, onValueChange = { host = it },
            label = { Text("Daemon IP") },
            singleLine = true, modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = port, onValueChange = { port = it.filter(Char::isDigit) },
            label = { Text("Port") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            modifier = Modifier.widthIn(min = 120.dp),
        )

        Button(
            onClick = {
                discovering = true
                scope.launch {
                    results = Discovery.discover(port.toIntOrNull() ?: 8025)
                    discovering = false
                }
            },
            enabled = !discovering,
            modifier = Modifier.fillMaxWidth(),
        ) {
            if (discovering) {
                CircularProgressIndicator(modifier = Modifier.padding(end = 8.dp))
                Text("Searching…")
            } else {
                Text("Discover on LAN")
            }
        }

        results.forEach { found ->
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(Panel)
                    .clickable { host = found.host; port = found.port.toString() }
                    .padding(12.dp),
            ) {
                Text(
                    found.console.ifEmpty { "Daemon" },
                    color = TextPrimary, fontWeight = FontWeight.SemiBold,
                )
                Text("${found.host}:${found.port}", color = Muted, fontSize = 12.sp)
            }
        }

        Button(
            onClick = {
                onConnect(Credentials(name.trim(), host.trim(), port.toIntOrNull() ?: 8025))
            },
            enabled = name.isNotBlank() && host.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Connect", fontWeight = FontWeight.SemiBold)
        }

        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
            Text(
                "On iPhone/iPad use the web monitor in a browser instead.",
                color = Muted, fontSize = 12.sp,
            )
        }
    }
}

package com.pob31.s21monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.PowerSettingsNew
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.pob31.s21monitor.model.MonitorUiState
import com.pob31.s21monitor.service.MonitorService
import com.pob31.s21monitor.ui.theme.Accent
import com.pob31.s21monitor.ui.theme.Bg
import com.pob31.s21monitor.ui.theme.Danger
import com.pob31.s21monitor.ui.theme.Muted
import com.pob31.s21monitor.ui.theme.OnAccent
import com.pob31.s21monitor.ui.theme.Panel
import com.pob31.s21monitor.ui.theme.Panel2
import com.pob31.s21monitor.ui.theme.TextPrimary
import com.pob31.s21monitor.ui.widgets.AuxStrip
import com.pob31.s21monitor.ui.widgets.SendStrip

@Composable
fun MonitorScreen(
    state: MonitorUiState,
    clientName: String,
    service: MonitorService,
    onShutdown: () -> Unit,
) {
    var tab by remember { mutableIntStateOf(0) } // 0 = My Mix, 1 = My Aux
    var selectedAux by remember { mutableStateOf<Int?>(null) }

    val auxes = state.availableAuxes
    LaunchedEffect(auxes) {
        if (selectedAux == null || selectedAux !in auxes) selectedAux = auxes.firstOrNull()
    }

    Column(Modifier.fillMaxSize().background(Bg)) {
        Header(
            console = state.console.ifEmpty { "S21 Monitor" },
            clientName = clientName,
            connected = state.connected,
            onShutdown = onShutdown,
        )
        TabsRow(
            tab = tab,
            onTab = { tab = it },
            showAuxPicker = tab == 0 && auxes.isNotEmpty(),
            auxes = auxes,
            auxStates = state,
            selectedAux = selectedAux,
            onSelectAux = { selectedAux = it },
        )
        Box(Modifier.fillMaxSize()) {
            if (tab == 0) MyMix(state, selectedAux, service) else MyAux(state, service)
        }
    }
}

@Composable
private fun Header(
    console: String,
    clientName: String,
    connected: Boolean,
    onShutdown: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().background(Panel).padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(console, color = TextPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        Text(clientName, color = Muted, fontSize = 13.sp)
        Box(Modifier.weight(1f))
        Box(
            Modifier.size(11.dp).clip(CircleShape)
                .background(if (connected) Accent else Danger),
        )
        Text(
            if (connected) "Connected" else "Connecting…",
            color = Muted, fontSize = 12.sp,
        )
        Icon(
            Icons.Filled.PowerSettingsNew,
            contentDescription = "Shut down",
            tint = Muted,
            modifier = Modifier.size(22.dp).clip(CircleShape).clickable(onClick = onShutdown),
        )
    }
}

@Composable
private fun TabsRow(
    tab: Int,
    onTab: (Int) -> Unit,
    showAuxPicker: Boolean,
    auxes: List<Int>,
    auxStates: MonitorUiState,
    selectedAux: Int?,
    onSelectAux: (Int) -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().background(Bg).padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Pill("My Mix", tab == 0) { onTab(0) }
        Pill("My Aux", tab == 1) { onTab(1) }
        Box(Modifier.weight(1f))
        if (showAuxPicker) {
            AuxPicker(auxes, auxStates, selectedAux, onSelectAux)
        }
    }
}

@Composable
private fun Pill(label: String, active: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .clip(RoundedCornerShape(20.dp))
            .background(if (active) Accent else Panel2)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Text(
            label,
            color = if (active) OnAccent else Muted,
            fontSize = 13.sp,
            fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal,
        )
    }
}

@Composable
private fun AuxPicker(
    auxes: List<Int>,
    state: MonitorUiState,
    selectedAux: Int?,
    onSelectAux: (Int) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    fun label(aux: Int?): String {
        if (aux == null) return "Select Aux"
        val name = state.auxes[aux]?.name.orEmpty()
        return if (name.isNotEmpty()) "$name (Aux $aux)" else "Aux $aux"
    }
    Box {
        Row(
            Modifier.clip(RoundedCornerShape(8.dp)).background(Panel2)
                .clickable { open = true }.padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(label(selectedAux), color = TextPrimary, fontSize = 13.sp)
            Icon(Icons.Filled.ExpandMore, contentDescription = null, tint = Muted)
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            auxes.forEach { a ->
                DropdownMenuItem(text = { Text(label(a)) }, onClick = {
                    onSelectAux(a); open = false
                })
            }
        }
    }
}

@Composable
private fun MyMix(state: MonitorUiState, selectedAux: Int?, service: MonitorService) {
    if (selectedAux == null) {
        Empty("Select an aux to start mixing"); return
    }
    val sends = state.sendsForAux(selectedAux)
    if (sends.isEmpty()) {
        Empty("Waiting for state from the daemon…"); return
    }
    Row(
        Modifier.fillMaxSize().horizontalScroll(rememberScrollState()).padding(10.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        sends.forEach { send ->
            SendStrip(
                send = send,
                onLevel = { service.setSendLevel(send.input, send.aux, it) },
                onPan = { service.setSendPan(send.input, send.aux, it) },
                onToggle = { service.setSendOn(send.input, send.aux, !send.on) },
            )
        }
    }
}

@Composable
private fun MyAux(state: MonitorUiState, service: MonitorService) {
    val auxes = state.availableAuxes
    if (auxes.isEmpty()) {
        Empty("No aux channels assigned"); return
    }
    Row(
        Modifier.fillMaxSize().horizontalScroll(rememberScrollState()).padding(10.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        auxes.forEach { a ->
            val aux = state.auxes[a] ?: com.pob31.s21monitor.model.AuxState(a)
            AuxStrip(
                aux = aux,
                onFader = { service.setAuxFader(a, it) },
                onMute = { service.setAuxMute(a, !aux.mute) },
            )
        }
    }
}

@Composable
private fun Empty(text: String) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text(text, color = Muted, fontSize = 14.sp)
    }
}

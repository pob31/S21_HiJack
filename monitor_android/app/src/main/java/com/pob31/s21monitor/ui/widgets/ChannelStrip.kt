package com.pob31.s21monitor.ui.widgets

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.pob31.s21monitor.model.AuxState
import com.pob31.s21monitor.model.SendState
import com.pob31.s21monitor.ui.theme.Accent
import com.pob31.s21monitor.ui.theme.Danger
import com.pob31.s21monitor.ui.theme.Muted
import com.pob31.s21monitor.ui.theme.OnAccent
import com.pob31.s21monitor.ui.theme.Panel
import com.pob31.s21monitor.ui.theme.Panel2
import com.pob31.s21monitor.ui.theme.TextPrimary

/** Vertical input-send strip for the "My Mix" tab. */
@Composable
fun SendStrip(
    send: SendState,
    onLevel: (Float) -> Unit,
    onPan: (Float) -> Unit,
    onToggle: () -> Unit,
    modifier: Modifier = Modifier,
) {
    StripFrame(
        title = send.name.ifEmpty { "In ${send.input}" },
        valueText = formatDb(send.level),
        width = 76.dp,
        active = send.on,
        modifier = modifier,
    ) {
        VerticalFader(
            db = send.level,
            active = send.on,
            onDb = onLevel,
            modifier = Modifier.fillMaxWidth().weight(1f),
        )
        Text(
            panLabel(send.pan),
            color = Muted,
            fontSize = 10.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
        PanControl(pan = send.pan, onPan = onPan, modifier = Modifier.fillMaxWidth())
        ToggleButton(
            label = "ON",
            active = send.on,
            activeColor = Accent,
            activeText = OnAccent,
            onClick = onToggle,
        )
    }
}

/** Wider aux-master strip for the "My Aux" tab. */
@Composable
fun AuxStrip(
    aux: AuxState,
    onFader: (Float) -> Unit,
    onMute: () -> Unit,
    modifier: Modifier = Modifier,
) {
    StripFrame(
        title = aux.name.ifEmpty { "Aux ${aux.aux}" },
        valueText = formatDb(aux.fader),
        width = 96.dp,
        active = !aux.mute,
        modifier = modifier,
    ) {
        VerticalFader(
            db = aux.fader,
            active = !aux.mute,
            onDb = onFader,
            modifier = Modifier.fillMaxWidth().weight(1f),
        )
        ToggleButton(
            label = "MUTE",
            active = aux.mute,
            activeColor = Danger,
            activeText = TextPrimary,
            onClick = onMute,
        )
    }
}

@Composable
private fun StripFrame(
    title: String,
    valueText: String,
    width: androidx.compose.ui.unit.Dp,
    active: Boolean,
    modifier: Modifier = Modifier,
    content: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit,
) {
    Column(
        modifier
            .width(width)
            .fillMaxHeight()
            .clip(RoundedCornerShape(10.dp))
            .background(Panel)
            .padding(8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(
            title,
            color = if (active) TextPrimary else Muted,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(valueText, color = Muted, fontSize = 11.sp)
        content()
    }
}

@Composable
private fun ToggleButton(
    label: String,
    active: Boolean,
    activeColor: androidx.compose.ui.graphics.Color,
    activeText: androidx.compose.ui.graphics.Color,
    onClick: () -> Unit,
) {
    Box(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(6.dp))
            .background(if (active) activeColor else Panel2)
            .clickable(onClick = onClick)
            .padding(vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            color = if (active) activeText else Muted,
            fontSize = 11.sp,
            fontWeight = FontWeight.Bold,
        )
    }
}

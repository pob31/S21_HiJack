package com.pob31.s21monitor.ui.widgets

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.dp
import com.pob31.s21monitor.ui.theme.Accent
import com.pob31.s21monitor.ui.theme.FaderFillBottom
import com.pob31.s21monitor.ui.theme.FaderFillTop
import com.pob31.s21monitor.ui.theme.FaderTrack
import com.pob31.s21monitor.ui.theme.Panel2
import com.pob31.s21monitor.ui.theme.TextPrimary
import com.pob31.s21monitor.ui.theme.Warn
import kotlin.math.max
import kotlin.math.min

/** Fader dB range (matches the web/Flutter clients). */
const val FADER_MIN_DB = -80f
const val FADER_MAX_DB = 10f

fun dbToFraction(db: Float): Float =
    ((db - FADER_MIN_DB) / (FADER_MAX_DB - FADER_MIN_DB)).coerceIn(0f, 1f)

fun fractionToDb(f: Float): Float =
    FADER_MIN_DB + f.coerceIn(0f, 1f) * (FADER_MAX_DB - FADER_MIN_DB)

fun formatDb(db: Float): String = if (db <= -59f) "-inf" else "%.1f dB".format(db)

/**
 * Vertical fader — a dark track filled bottom-up with the web monitor's blue
 * gradient. Drag (or touch) anywhere on the track to set the level. Dimmed
 * when [active] is false (off / muted).
 */
@Composable
fun VerticalFader(
    db: Float,
    active: Boolean,
    onDb: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    var heightPx by remember { mutableFloatStateOf(1f) }
    val frac = dbToFraction(db)

    fun setFromY(y: Float) {
        val f = (1f - y / heightPx).coerceIn(0f, 1f)
        onDb(fractionToDb(f))
    }

    Box(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(FaderTrack)
            .onSizeChanged { heightPx = it.height.toFloat().coerceAtLeast(1f) }
            .pointerInput(Unit) {
                detectVerticalDragGestures(
                    onDragStart = { setFromY(it.y) },
                    onVerticalDrag = { change, _ ->
                        change.consume()
                        setFromY(change.position.y)
                    },
                )
            },
        contentAlignment = Alignment.BottomCenter,
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(frac)
                .alpha(if (active) 1f else 0.35f)
                .background(Brush.verticalGradient(listOf(FaderFillTop, FaderFillBottom))),
            contentAlignment = Alignment.TopCenter,
        ) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(3.dp)
                    .alpha(if (active) 1f else 0.4f)
                    .background(if (active) TextPrimary else Accent),
            )
        }
    }
}

/**
 * Bidirectional pan slider — the amber fill grows from the centre out to the
 * thumb (left of centre = pan L, right = pan R), matching the web monitor's
 * `panGradient`. Drag to set; double-tap to recentre.
 */
@Composable
fun PanControl(
    pan: Float,
    onPan: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    var widthPx by remember { mutableFloatStateOf(1f) }

    fun setFromX(x: Float) {
        val f = (x / widthPx).coerceIn(0f, 1f)
        onPan((f * 2f - 1f).coerceIn(-1f, 1f))
    }

    Canvas(
        modifier
            .fillMaxWidth()
            .height(24.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(Panel2)
            .onSizeChanged { widthPx = it.width.toFloat().coerceAtLeast(1f) }
            .pointerInput(Unit) {
                detectHorizontalDragGestures(
                    onDragStart = { setFromX(it.x) },
                    onHorizontalDrag = { change, _ ->
                        change.consume()
                        setFromX(change.position.x)
                    },
                )
            }
            .pointerInput(Unit) {
                detectTapGestures(onDoubleTap = { onPan(0f) })
            },
    ) {
        val w = size.width
        val h = size.height
        val frac = (pan.coerceIn(-1f, 1f) + 1f) / 2f
        val center = w * 0.5f
        val pos = w * frac
        val lo = min(pos, center)
        val hi = max(pos, center)
        // Amber fill from centre to the thumb (bidirectional).
        drawRect(color = Warn, topLeft = Offset(lo, 0f), size = Size(hi - lo, h))
        // Custom thumb.
        val tw = 10.dp.toPx()
        val tx = (pos - tw / 2f).coerceIn(0f, w - tw)
        drawRect(color = TextPrimary, topLeft = Offset(tx, 0f), size = Size(tw, h))
    }
}

fun panLabel(pan: Float): String = when {
    pan in -0.02f..0.02f -> "C"
    pan < 0 -> "L${(-pan * 100).toInt()}"
    else -> "R${(pan * 100).toInt()}"
}

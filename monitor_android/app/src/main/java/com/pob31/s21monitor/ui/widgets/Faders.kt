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
import androidx.compose.runtime.rememberUpdatedState
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

/** Fader dB range, with a linear taper. (The web client's floor is −60 dB.) */
const val FADER_MIN_DB = -80f
const val FADER_MAX_DB = 10f

fun dbToFraction(db: Float): Float =
    ((db - FADER_MIN_DB) / (FADER_MAX_DB - FADER_MIN_DB)).coerceIn(0f, 1f)

fun formatDb(db: Float): String = if (db <= -59f) "-inf" else "%.1f dB".format(db)

/**
 * The level during a relative fader drag: [startDb], the level when the drag
 * began, moved by the finger's vertical travel since, where the fader's full
 * height spans the full dB range. [travelY] is in px and positive downwards
 * (screen y), so dragging up raises the level. A start below the fader's
 * floor (the daemon's −150 dB "off") counts as the floor.
 */
fun dragDb(startDb: Float, travelY: Float, heightPx: Float): Float {
    val start = if (startDb.isFinite()) startDb.coerceIn(FADER_MIN_DB, FADER_MAX_DB) else FADER_MIN_DB
    val span = FADER_MAX_DB - FADER_MIN_DB
    return (start - travelY / heightPx.coerceAtLeast(1f) * span).coerceIn(FADER_MIN_DB, FADER_MAX_DB)
}

/**
 * Vertical fader — a dark track filled bottom-up with the web monitor's blue
 * gradient. Dimmed when [active] is false (off / muted).
 *
 * Dragging is relative, like the web client's fader: the level moves by the
 * finger's travel from wherever it was, and never jumps to where the finger
 * lands (audit A2). A drag starts only if the finger passes the touch slop
 * vertically before the strip row's scroller sees as much horizontal travel,
 * so a mostly horizontal swipe still scrolls the row (the web client's
 * direction lock).
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
    // The gesture below outlives recompositions, so it reads the level and the
    // callback through these instead of keeping the first ones (audit A1).
    val currentDb by rememberUpdatedState(db)
    val currentOnDb by rememberUpdatedState(onDb)

    Box(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(FaderTrack)
            .onSizeChanged { heightPx = it.height.toFloat().coerceAtLeast(1f) }
            .pointerInput(Unit) {
                var startDb = 0f
                var travelY = 0f
                var lastDb = 0f
                detectVerticalDragGestures(
                    onDragStart = {
                        startDb = currentDb
                        travelY = 0f
                        lastDb = dragDb(startDb, 0f, heightPx)
                    },
                    onVerticalDrag = { change, dragAmount ->
                        change.consume()
                        travelY += dragAmount
                        val v = dragDb(startDb, travelY, heightPx)
                        // Send only when the level moves: not while pinned at
                        // an end of travel, and not for a downward drag on a
                        // fader that's off (that would send the −80 dB floor).
                        if (v != lastDb) {
                            lastDb = v
                            currentOnDb(v)
                        }
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
    // The gestures below outlive recompositions (audit A1).
    val currentOnPan by rememberUpdatedState(onPan)

    fun setFromX(x: Float) {
        val f = (x / widthPx).coerceIn(0f, 1f)
        currentOnPan((f * 2f - 1f).coerceIn(-1f, 1f))
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
                detectTapGestures(onDoubleTap = { currentOnPan(0f) })
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

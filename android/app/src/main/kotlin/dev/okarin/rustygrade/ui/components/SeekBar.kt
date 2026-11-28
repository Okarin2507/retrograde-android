package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import kotlin.math.abs
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Slim desktop-style continuous seek strip with `mm:ss` labels underneath.
 * Reads [PlaybackHub] directly so only this composable recomposes on position ticks.
 */
@Composable
fun SeekBar(onSeek: (Long) -> Unit, modifier: Modifier = Modifier, fallbackDurationMs: Long = 0L) {
    val positionMs by PlaybackHub.positionMs.collectAsStateWithLifecycle()
    val reportedMs by PlaybackHub.durationMs.collectAsStateWithLifecycle()
    val durationMs = if (reportedMs > 0) reportedMs else fallbackDurationMs
    val seek = rememberUpdatedState(onSeek)
    val duration = rememberUpdatedState(durationMs)

    // Fraction while the finger is down, held briefly after release until the player catches up.
    var held by remember { mutableStateOf<Float?>(null) }
    var dragging by remember { mutableStateOf(false) }
    val position = rememberUpdatedState(positionMs)
    // After a release, keep the held fraction until the player catches up (or 800 ms pass). Keyed
    // on the hold only, so position ticks do not restart an effect while nothing is held.
    LaunchedEffect(held, dragging) {
        val h = held ?: return@LaunchedEffect
        if (dragging) return@LaunchedEffect
        withTimeoutOrNull(800) {
            snapshotFlow { position.value }.first { p ->
                val d = duration.value
                d > 0 && abs(p - h * d) < 1500
            }
        }
        held = null
    }

    val liveFraction = if (durationMs > 0) (positionMs.toFloat() / durationMs).coerceIn(0f, 1f) else 0f
    val fraction = held ?: liveFraction
    val shownMs = if (held != null) (held!! * durationMs).toLong() else positionMs
    val filledColor = rememberReadableAccentOn(Orbit.Void)
    val trackColor = Orbit.Text.withAlpha(Alpha.Tint)

    Column(modifier.fillMaxWidth()) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(Dimens.Touch)
                .semantics(mergeDescendants = true) {
                    contentDescription = "Playback position"
                    progressBarRangeInfo = ProgressBarRangeInfo(fraction, 0f..1f)
                    stateDescription = "${formatDuration(shownMs / 1000.0)} of ${formatDuration(durationMs / 1000.0)}"
                    setProgress { target ->
                        val d = duration.value
                        if (d > 0) {
                            val f = target.coerceIn(0f, 1f)
                            held = f
                            seek.value((f * d).toLong())
                            true
                        } else {
                            false
                        }
                    }
                }
                .pointerInput(Unit) {
                    detectTapGestures { offset ->
                        val d = duration.value
                        if (d > 0 && size.width > 0) {
                            val f = (offset.x / size.width).coerceIn(0f, 1f)
                            held = f
                            seek.value((f * d).toLong())
                        }
                    }
                }
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = { offset ->
                            if (duration.value > 0 && size.width > 0) {
                                dragging = true
                                held = (offset.x / size.width).coerceIn(0f, 1f)
                            }
                        },
                        onDragEnd = {
                            val f = held
                            dragging = false
                            if (f != null) seek.value((f * duration.value).toLong())
                        },
                        onDragCancel = {
                            dragging = false
                            held = null
                        },
                        onHorizontalDrag = { change, _ ->
                            change.consume()
                            if (dragging && size.width > 0) {
                                held = (change.position.x / size.width).coerceIn(0f, 1f)
                            }
                        },
                    )
                },
        ) {
            val trackH = if (dragging) 5.dp.toPx() else 3.dp.toPx()
            val trackY = (size.height - trackH) / 2f
            val radius = CornerRadius(trackH / 2f, trackH / 2f)

            // Full track background
            drawRoundRect(
                color = trackColor,
                topLeft = Offset(0f, trackY),
                size = Size(size.width, trackH),
                cornerRadius = radius,
            )

            // Played progress fill
            val fillW = (size.width * fraction).coerceIn(0f, size.width)
            if (fillW > 0.5f) {
                drawRoundRect(
                    color = filledColor,
                    topLeft = Offset(0f, trackY),
                    size = Size(fillW, trackH),
                    cornerRadius = radius,
                )
            }

            // Crisp Lonetrail vertical cursor knob
            val knobW = if (dragging) 5.dp.toPx() else 3.dp.toPx()
            val knobH = if (dragging) Space.L.toPx() else Space.M.toPx()
            val knobX = (fillW - knobW / 2f).coerceIn(0f, (size.width - knobW).coerceAtLeast(0f))
            val knobY = (size.height - knobH) / 2f
            drawRoundRect(
                color = if (dragging) Orbit.Text else filledColor,
                topLeft = Offset(knobX, knobY),
                size = Size(knobW, knobH),
                cornerRadius = CornerRadius(Dimens.Stroke.toPx(), Dimens.Stroke.toPx()),
            )
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Text(formatDuration(shownMs / 1000.0), color = if (held != null) filledColor else Orbit.TextDim, style = OrbitType.Numeric)
            Text(formatDuration(durationMs / 1000.0), color = Orbit.TextDim, style = OrbitType.Numeric)
        }
    }
}

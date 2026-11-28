package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.Space
import kotlin.math.round

/** Geometric Orbit slider: flat track, optional step ticks and sharp rectangular thumb. */
@Composable
fun OrbitSlider(
    value: Float,
    valueRange: ClosedFloatingPointRange<Float>,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    onValueChangeFinished: (() -> Unit)? = null,
    steps: Int = 0,
    accent: Color = Orbit.Teal,
    contentDescription: String? = null,
) {
    val span = (valueRange.endInclusive - valueRange.start).coerceAtLeast(1e-6f)
    val clampedValue = value.coerceIn(valueRange.start, valueRange.endInclusive)
    val fraction = ((clampedValue - valueRange.start) / span).coerceIn(0f, 1f)
    val latestOnChange by rememberUpdatedState(onValueChange)
    val latestOnFinished by rememberUpdatedState(onValueChangeFinished)

    fun snapFraction(raw: Float): Float {
        val clamped = raw.coerceIn(0f, 1f)
        return if (steps > 0) {
            val segments = steps + 1
            round(clamped * segments) / segments.toFloat()
        } else {
            clamped
        }
    }

    fun mapOffsetToValue(x: Float, width: Float, thumbPx: Float, padPx: Float): Float {
        val usable = (width - thumbPx - padPx * 2f).coerceAtLeast(1f)
        val raw = (x - padPx - thumbPx / 2f) / usable
        return valueRange.start + snapFraction(raw) * span
    }

    Canvas(
        modifier
            .fillMaxWidth()
            .height(Dimens.Touch)
            .semantics(mergeDescendants = true) {
                if (contentDescription != null) this.contentDescription = contentDescription
                progressBarRangeInfo = ProgressBarRangeInfo(clampedValue, valueRange, steps)
                setProgress { target ->
                    val raw = ((target - valueRange.start) / span).coerceIn(0f, 1f)
                    val snappedValue = valueRange.start + snapFraction(raw) * span
                    if (snappedValue == clampedValue) {
                        false
                    } else {
                        latestOnChange(snappedValue)
                        latestOnFinished?.invoke()
                        true
                    }
                }
            }
            .pointerInput(valueRange, steps) {
                val thumbPx = Space.M.toPx()
                val padPx = Dimens.Stroke.toPx()
                detectTapGestures { offset ->
                    latestOnChange(mapOffsetToValue(offset.x, size.width.toFloat(), thumbPx, padPx))
                    latestOnFinished?.invoke()
                }
            }
            .pointerInput(valueRange, steps) {
                val thumbPx = Space.M.toPx()
                val padPx = Dimens.Stroke.toPx()
                detectHorizontalDragGestures(
                    onDragStart = { offset ->
                        latestOnChange(mapOffsetToValue(offset.x, size.width.toFloat(), thumbPx, padPx))
                    },
                    onHorizontalDrag = { change, _ ->
                        change.consume()
                        latestOnChange(mapOffsetToValue(change.position.x, size.width.toFloat(), thumbPx, padPx))
                    },
                    onDragEnd = { latestOnFinished?.invoke() },
                    onDragCancel = { latestOnFinished?.invoke() },
                )
            },
    ) {
        val pad = Dimens.Stroke.toPx()
        val thumbW = Space.M.toPx()
        val thumbH = Dimens.IconL.toPx()
        val trackH = Space.Xs.toPx()
        val usableW = (size.width - thumbW - pad * 2f).coerceAtLeast(1f)
        val trackLeft = pad + thumbW / 2f
        val trackTop = (size.height - trackH) / 2f
        val fillW = usableW * fraction

        drawRect(Orbit.Line, Offset(trackLeft, trackTop), Size(usableW, trackH))
        drawRect(accent, Offset(trackLeft, trackTop), Size(fillW, trackH))
        if (steps in 1..24) {
            val segments = steps + 1
            val tickW = Dimens.Rule.toPx()
            val tickH = Space.S.toPx()
            val tickTop = (size.height - tickH) / 2f
            for (i in 0..segments) {
                val tx = trackLeft + usableW * (i / segments.toFloat()) - tickW / 2f
                val active = i / segments.toFloat() <= fraction + 0.001f
                drawRect(if (active) accent else Orbit.Line, Offset(tx, tickTop), Size(tickW, tickH))
            }
        }
        val thumbLeft = pad + fillW
        val thumbTop = (size.height - thumbH) / 2f
        drawRect(Orbit.Void, Offset(thumbLeft - pad, thumbTop - pad), Size(thumbW + pad * 2f, thumbH + pad * 2f))
        drawRect(accent, Offset(thumbLeft, thumbTop), Size(thumbW, thumbH))
    }
}

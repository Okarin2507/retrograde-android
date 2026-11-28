package dev.okarin.rustygrade.ui.motion

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha

/**
 * The desktop `pulse_dot`: a square status dot with a soft halo that breathes while [active]
 * (something is playing). Static when paused or when motion is off, so it costs nothing idle.
 */
@Composable
fun PulseDot(color: Color, modifier: Modifier = Modifier, size: Dp = Space.Compact, active: Boolean = true) {
    val animate = active && LocalMotionEnabled.current
    // Kept as a State and read only inside the draw lambda: the pulse redraws, never recomposes.
    val phaseState: State<Float>? = if (animate) {
        rememberInfiniteTransition(label = "pulse").animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(Motion.PulseMs, easing = Motion.Ease.CubicInOut), RepeatMode.Reverse),
            label = "pulse-phase",
        )
    } else {
        null
    }
    Canvas(modifier.size(size * 2.4f)) {
        val core = size.toPx()
        val c = Offset(this.size.width / 2, this.size.height / 2)
        if (phaseState != null) {
            val phase = phaseState.value
            val halo = core * (1f + 0.7f * phase)
            drawRect(
                color.withAlpha(Alpha.Faint * (1f - phase)),
                topLeft = Offset(c.x - halo / 2, c.y - halo / 2),
                size = androidx.compose.ui.geometry.Size(halo, halo),
            )
        }
        drawRect(
            if (active) color else color.withAlpha(Alpha.Muted),
            topLeft = Offset(c.x - core / 2, c.y - core / 2),
            size = androidx.compose.ui.geometry.Size(core, core),
        )
    }
}

/** A steady square status LED (no animation), sized like [PulseDot]'s core. */
@Composable
fun StatusDot(color: Color, modifier: Modifier = Modifier, size: Dp = Space.Compact) {
    Canvas(modifier.size(size * 2.4f)) {
        val core = size.toPx()
        val c = Offset(this.size.width / 2, this.size.height / 2)
        drawRect(color, topLeft = Offset(c.x - core / 2, c.y - core / 2), size = androidx.compose.ui.geometry.Size(core, core))
    }
}

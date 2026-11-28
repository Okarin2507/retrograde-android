package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.GenericShape
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Dp
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.pressScale
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.readableTextOn
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin

/** Flat-top hexagon inscribed in the component bounds. */
private val HexShape = GenericShape { size, _ ->
    val r = min(size.width, size.height) / 2f
    val cx = size.width / 2f
    val cy = size.height / 2f
    for (i in 0 until 6) {
        val a = (PI / 3.0 * i).toFloat()
        val x = cx + r * cos(a)
        val y = cy + r * sin(a)
        if (i == 0) moveTo(x, y) else lineTo(x, y)
    }
    close()
}

/** Scale of the hex while held (`HEX_PRESS`). */
private const val PressedScale = 0.92f

/**
 * Hexagonal play/pause button: accent fill while [playing], accent outline while paused. It shrinks
 * while held (`HEX_PRESS` in, `HEX_HOVER` out) and snaps instead when motion is off.
 */
@Composable
fun HexButton(
    playing: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    size: Dp = Dimens.Touch,
    accent: Color = LocalAccent.current,
) {
    // Paused: outline and glyph sit on the panel, so a dark accent is lifted until it is readable.
    val outline = if (playing) accent else rememberReadableAccentOn(Orbit.Panel, accent)
    // The scale sits before the clip so the whole hexagon (fill, outline and ripple) shrinks together.
    val source = remember { MutableInteractionSource() }
    val base = modifier
        .size(size)
        .pressScale(source, pressedScale = PressedScale)
        .clip(HexShape)
        .let { if (playing) it.background(accent) else it.border(Dimens.Rule, outline, HexShape) }
        .clickable(interactionSource = source, indication = LocalIndication.current, role = Role.Button, onClick = onClick)
    Box(base, contentAlignment = Alignment.Center) {
        Icon(
            if (playing) OrbitIcons.Pause else OrbitIcons.Play,
            contentDescription = if (playing) "Pause" else "Play",
            tint = if (playing) readableTextOn(accent) else outline,
            modifier = Modifier.size(size * 0.42f),
        )
    }
}

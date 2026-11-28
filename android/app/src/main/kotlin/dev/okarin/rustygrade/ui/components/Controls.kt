package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.layout
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha
import kotlin.math.max

/** Outlined square mode chip (ALBUMS / ARTISTS ...): gold when on. */
@Composable
fun OrbitChip(
    label: String,
    active: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Text(
        label,
        modifier = modifier
            // A 44 dp touch target; the drawn pill stays centered inside it.
            .pressable(pressedScale = 0.95f, onClick = onClick)
            .minimumTouchTarget()
            .border(BorderStroke(Dimens.Stroke, if (active) Orbit.Gold else Orbit.Line))
            .background(if (active) Orbit.Gold.withAlpha(Alpha.Tint) else Orbit.Panel)
            .padding(horizontal = Dimens.RowGap, vertical = Space.M),
        color = if (active) Orbit.Gold else Orbit.TextDim,
        style = OrbitType.Chip,
        maxLines = 1,
    )
}

/** Grows the touch area to at least [size] x [size] without growing what is drawn inside it. */
private fun Modifier.minimumTouchTarget(size: Dp = Dimens.Touch): Modifier = this.layout { measurable, constraints ->
    val placeable = measurable.measure(constraints)
    val minPx = size.roundToPx()
    val w = max(placeable.width, minPx).coerceIn(constraints.minWidth, constraints.maxWidth)
    val h = max(placeable.height, minPx).coerceIn(constraints.minHeight, constraints.maxHeight)
    layout(w, h) { placeable.place((w - placeable.width) / 2, (h - placeable.height) / 2) }
}

/** A 44 dp icon button: [icon] is drawn at [iconSize] in the middle of the touch target. */
@Composable
fun IconTap(
    icon: ImageVector,
    description: String,
    tint: Color,
    modifier: Modifier = Modifier,
    iconSize: Dp = Dimens.IconL,
    stateDescription: String? = null,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .size(Dimens.Touch)
            .pressable(pressedScale = 0.9f, onClick = onClick)
            .then(if (stateDescription != null) Modifier.semantics { this.stateDescription = stateDescription } else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, description, tint = tint, modifier = Modifier.size(iconSize))
    }
}

/** Primary actions are an off-white block with dark text, like the desktop play button. */
@Composable
fun OrbitButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    danger: Boolean = false,
    icon: ImageVector? = null,
    enabled: Boolean = true,
) {
    val fg = when {
        !enabled -> Orbit.TextMute
        primary -> Orbit.Void
        danger -> Orbit.Danger
        else -> Orbit.Text
    }
    val bg = when {
        primary && enabled -> Orbit.Text
        primary && !enabled -> Orbit.Card
        danger && enabled -> Orbit.Danger.withAlpha(Alpha.Wash)
        else -> Orbit.Panel
    }
    val borderColor = when {
        primary && enabled -> Orbit.Text
        primary && !enabled -> Orbit.LineSoft
        danger && enabled -> Orbit.Danger
        !enabled -> Orbit.LineSoft
        else -> Orbit.Line
    }
    Row(
        modifier
            .heightIn(min = Dimens.Touch)
            .background(bg)
            .border(BorderStroke(Dimens.Stroke, borderColor))
            .then(if (enabled) Modifier.pressable(onClick = onClick) else Modifier)
            .padding(horizontal = Dimens.RowGap, vertical = Dimens.RowPadV),
        horizontalArrangement = Arrangement.spacedBy(Space.S, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) Icon(icon, null, tint = fg, modifier = Modifier.size(Dimens.IconS))
        Text(
            label,
            color = fg,
            style = OrbitType.Button,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

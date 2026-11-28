package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.Kicker
import dev.okarin.rustygrade.ui.motion.pressScale
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha

/**
 * A HUD console panel: a teal `— KICKER` [title] above an `Orbit.Panel` block with a hairline
 * border and the desktop orange corner tick in its top-left corner, holding [content].
 */
@Composable
internal fun SettingsPanel(
    title: String,
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.S)) {
        Kicker(title)
        Box(Modifier.fillMaxWidth()) {
            Column(
                Modifier
                    .fillMaxWidth()
                    .background(Orbit.Panel)
                    .border(BorderStroke(Dimens.Stroke, Orbit.LineSoft))
                    .padding(horizontal = Space.L, vertical = Space.M),
                verticalArrangement = Arrangement.spacedBy(Space.M),
                content = content,
            )
            // Drawn last so the tick sits over the border; a Canvas takes no input.
            CornerBrackets(Modifier.matchParentSize(), length = Dimens.TickS, bottomRight = null)
        }
    }
}

/** Material3 [Switch] in Orbit colors: teal when on. */
@Composable
/** Pass a null [onCheckedChange] when a parent (e.g. [ToggleRow]) owns the toggle gesture. */
internal fun OrbitSwitch(checked: Boolean, onCheckedChange: ((Boolean) -> Unit)?) {
    Switch(
        checked = checked,
        onCheckedChange = onCheckedChange,
        colors = SwitchDefaults.colors(
            checkedThumbColor = Orbit.Teal,
            checkedTrackColor = Orbit.Teal.withAlpha(Alpha.Faint),
            checkedBorderColor = Orbit.Teal,
            uncheckedThumbColor = Orbit.TextDim,
            uncheckedTrackColor = Orbit.Void,
            uncheckedBorderColor = Orbit.Line,
        ),
    )
}

/** A label (and optional dim [caption]) on the left, an [OrbitSwitch] on the right. */
@Composable
internal fun ToggleRow(
    label: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    caption: String? = null,
) {
    // One toggleable node for the whole row: TalkBack reads the label and the state together.
    // The press scale is driven from the toggle's own interaction source so the Switch role stays.
    val source = remember { MutableInteractionSource() }
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = Dimens.Touch)
            .pressScale(source, pressedScale = 0.985f)
            .toggleable(
                value = checked,
                interactionSource = source,
                indication = LocalIndication.current,
                role = Role.Switch,
                onValueChange = onCheckedChange,
            ),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.M),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
            SettingLabel(label)
            if (caption != null) Caption(caption)
        }
        OrbitSwitch(checked, null)
    }
}

/** The bright tracked mono label of a setting row (`CROSSFADE`, `REPLAYGAIN`). */
@Composable
internal fun SettingLabel(text: String, modifier: Modifier = Modifier, color: Color = Orbit.Text) {
    Text(text, modifier, color = color, style = OrbitType.Button)
}

/** A readable secondary caption line. */
@Composable
internal fun Caption(text: String, modifier: Modifier = Modifier, color: Color = Orbit.TextDim) {
    Text(text, modifier, color = color, style = OrbitType.RowMeta)
}

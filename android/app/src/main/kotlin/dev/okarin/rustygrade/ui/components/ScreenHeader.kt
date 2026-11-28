package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space

/**
 * Screen title row: the desktop `LIBRARY_DATA` heading (accent bar + tracked mono caps) or a back
 * arrow on pushed screens, with an optional teal [status] line under the title (counts, state)
 * and trailing [actions].
 */
@Composable
fun ScreenHeader(
    title: String,
    modifier: Modifier = Modifier,
    onBack: (() -> Unit)? = null,
    status: String? = null,
    actions: @Composable () -> Unit = {},
) {
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = Dimens.HeaderHeight)
            .padding(start = if (onBack != null) Space.Xs else Space.Gutter, end = Space.Xs, top = Space.Compact, bottom = Space.Compact),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.S),
    ) {
        if (onBack != null) {
            Box(
                Modifier.size(Dimens.Touch).pressable(pressedScale = 0.9f, onClick = onBack),
                contentAlignment = Alignment.Center,
            ) {
                Icon(OrbitIcons.Arrowleft, "Back", tint = Orbit.Text, modifier = Modifier.size(Dimens.IconM))
            }
        } else {
            Box(Modifier.width(Dimens.TitleBarWidth).height(if (status != null) 30.dp else Dimens.TitleBarHeight).background(LocalAccent.current))
        }
        Column(Modifier.weight(1f)) {
            Text(
                title.uppercase(),
                color = Orbit.Text,
                style = OrbitType.ScreenTitle,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (status != null) {
                Text(
                    status.uppercase(),
                    color = Orbit.Teal,
                    style = OrbitType.Kicker,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        actions()
    }
}

/** Centered note for empty lists. */
@Composable
fun EmptyMessage(text: String, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().padding(Space.Xxxl), contentAlignment = Alignment.Center) {
        Text(
            text.uppercase(),
            color = Orbit.TextDim,
            style = OrbitType.Hud,
            textAlign = TextAlign.Center,
        )
    }
}

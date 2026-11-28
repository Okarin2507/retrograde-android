package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.IconTap
import dev.okarin.rustygrade.ui.components.OrbitTextField
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha
import java.util.Locale

/** Corner-tick overlay sized for a small row tile: arms scale down so a 48dp tile is not all bracket. */
@Composable
private fun TileTicks(size: Dp, modifier: Modifier = Modifier) {
    CornerBrackets(
        modifier.padding(Space.Xs),
        length = minOf(Dimens.CornerTick, size * 0.22f),
    )
}

/**
 * A square block holding an icon, used as the "cover" of Favorites and smart playlists. With
 * [ticks] it gets the desktop corner-tick frame and a faint wash of [tint], like a framed cover.
 */
@Composable
internal fun IconBlock(icon: ImageVector, tint: Color, size: Dp, modifier: Modifier = Modifier, ticks: Boolean = false) {
    Box(
        modifier
            .size(size)
            .background(Orbit.Card)
            .then(if (ticks) Modifier.background(tint.withAlpha(Alpha.Wash)) else Modifier)
            .border(BorderStroke(Dimens.Stroke, Orbit.Line)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = tint, modifier = Modifier.size(size * 0.4f))
        if (ticks) TileTicks(size, Modifier.matchParentSize())
    }
}

/** Playlist cover in a row: hairline frame plus the corner ticks of the album cards. */
@Composable
internal fun FramedCover(path: String?, size: Dp, modifier: Modifier = Modifier) {
    Box(modifier.size(size).border(BorderStroke(Dimens.Stroke, Orbit.Line))) {
        Cover(path, Modifier.fillMaxSize(), bordered = false)
        TileTicks(size, Modifier.matchParentSize())
    }
}

/** Dropdown of `(key, label)` options styled like [OrbitTextField]; gold while open. */
@Composable
internal fun OrbitDropdown(
    options: List<Pair<String, String>>,
    selectedKey: String,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    var open by remember { mutableStateOf(false) }
    val label = options.firstOrNull { it.first == selectedKey }?.second ?: selectedKey
    Box(modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(Orbit.Void)
                .border(BorderStroke(Dimens.Stroke, if (open) Orbit.Gold else Orbit.Line))
                .clickable { open = true }
                .heightIn(min = Dimens.FieldHeight)
                .padding(horizontal = Space.M),
            horizontalArrangement = Arrangement.spacedBy(Space.S),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                label.uppercase(),
                modifier = Modifier.weight(1f),
                color = if (open) Orbit.Gold else Orbit.Text,
                style = OrbitType.Hud,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Icon(OrbitIcons.Chevrondown, null, tint = if (open) Orbit.Gold else Orbit.TextDim, modifier = Modifier.size(Dimens.IconS))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = Orbit.Raised) {
            options.forEach { (key, text) ->
                DropdownMenuItem(
                    text = {
                        Text(
                            text.uppercase(),
                            color = if (key == selectedKey) Orbit.Gold else Orbit.Text,
                            style = if (key == selectedKey) OrbitType.HudBold else OrbitType.Hud,
                        )
                    },
                    onClick = {
                        open = false
                        onSelect(key)
                    },
                )
            }
        }
    }
}

/** Dialog asking for a playlist name; the confirm button is disabled while the name is blank. */
@Composable
internal fun NameDialog(
    title: String,
    initial: String,
    confirmLabel: String,
    onConfirm: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var name by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        shape = RectangleShape,
        containerColor = Orbit.Raised,
        title = {
            Text(
                title.uppercase(),
                color = Orbit.Gold,
                style = OrbitType.ScreenTitle,
            )
        },
        text = { OrbitTextField(name, { name = it }, Modifier.fillMaxWidth(), placeholder = "PLAYLIST NAME") },
        confirmButton = {
            val ok = name.isNotBlank()
            TextButton(onClick = { onConfirm(name.trim()) }, enabled = ok) {
                Text(confirmLabel, color = if (ok) Orbit.Gold else Orbit.TextMute, style = OrbitType.Button)
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("CANCEL", color = Orbit.TextDim, style = OrbitType.Button) }
        },
    )
}

/** `1H 23M` / `45M` for a total length in seconds. */
internal fun totalLabel(seconds: Double): String {
    val minutes = (seconds / 60).toLong().coerceAtLeast(0)
    val h = minutes / 60
    val m = minutes % 60
    return if (h > 0) "${h}H %02dM".format(Locale.US, m) else "${m}M"
}

/** Inline banner for playlist import/export status shared between [PlaylistsScreen] and [PlaylistDetailScreen]. */
@Composable
internal fun IoStatusBanner(message: String, accent: Color, onDismiss: (() -> Unit)?) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = Space.Gutter, vertical = Space.Xs)
            .background(Orbit.Panel)
            .border(BorderStroke(Dimens.Stroke, accent))
            .padding(start = Space.M, top = Space.S, bottom = Space.S, end = if (onDismiss != null) Space.Xs else Space.M),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.S),
    ) {
        Text(
            message,
            modifier = Modifier.weight(1f),
            color = accent,
            style = OrbitType.Hud,
        )
        if (onDismiss != null) {
            IconTap(OrbitIcons.X, "Dismiss", Orbit.TextDim, iconSize = Dimens.IconS, onClick = onDismiss)
        }
    }
}

/**
 * Maps a drag/move `[fromVisible] -> [toVisible]` over resolved [visibleIds] onto indices in
 * [rawIds], which may still include unindexed or offline track IDs.
 */
internal fun resolveRawMoveIndices(
    rawIds: List<String>,
    visibleIds: List<String>,
    fromVisible: Int,
    toVisible: Int,
): Pair<Int, Int>? {
    if (fromVisible == toVisible) return null
    if (fromVisible !in visibleIds.indices || toVisible !in visibleIds.indices) return null
    val fromTarget = visibleIds[fromVisible]
    val toTarget = visibleIds[toVisible]
    val fromOccurrence = visibleIds.take(fromVisible + 1).count { it == fromTarget }
    val toOccurrence = visibleIds.take(toVisible + 1).count { it == toTarget }
    val rawFrom = nthIndexOf(rawIds, fromTarget, fromOccurrence) ?: return null
    val rawTo = nthIndexOf(rawIds, toTarget, toOccurrence) ?: return null
    return if (rawFrom == rawTo) null else rawFrom to rawTo
}

private fun nthIndexOf(list: List<String>, target: String, n: Int): Int? {
    var seen = 0
    list.forEachIndexed { index, value ->
        if (value == target) {
            seen++
            if (seen == n) return index
        }
    }
    return null
}

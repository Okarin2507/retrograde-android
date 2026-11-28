package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.PulseDot
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import uniffi.rustygrade_ffi.TrackInfo

/**
 * One track in a list. The playing track is drawn in the accent color with an accent edge and a
 * pulsing dot in place of its [number] (track number or position). Hearts show favorites.
 */
@Composable
fun TrackRow(
    track: TrackInfo,
    onClick: () -> Unit,
    onMore: () -> Unit,
    modifier: Modifier = Modifier,
    number: String? = null,
    showCover: Boolean = false,
) {
    val isCurrent = LocalNowPlayingId.current == track.id
    // Only the playing row reads the accent, so an accent fade recomposes one row, not the list.
    val accent = if (isCurrent) rememberReadableAccentOn(Orbit.Void, LocalAccent.current) else Orbit.Text
    val isLiked = track.id in LocalLikedIds.current
    Column(modifier.fillMaxWidth()) {
        Box(Modifier.fillMaxWidth()) {
            if (isCurrent) {
                // A faint accent wash fading out to the right, under an accent edge.
                val wash = remember(accent) {
                    Brush.horizontalGradient(listOf(accent.withAlpha(Alpha.Wash), Orbit.Void.withAlpha(0f)))
                }
                Box(Modifier.matchParentSize().background(wash))
                Box(Modifier.align(Alignment.CenterStart).width(Dimens.Rule).height(28.dp).background(accent))
            }
            Row(
                Modifier
                    .fillMaxWidth()
                    .pressable(pressedScale = 0.985f, onClick = onClick)
                    .padding(start = Space.Gutter, end = Space.Xs, top = Dimens.RowPadV, bottom = Dimens.RowPadV),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Space.M),
            ) {
                if (showCover) {
                    Cover(track.thumbPath, size = Dimens.RowCover)
                } else if (number != null) {
                    Box(
                        Modifier
                            .widthIn(min = Space.Xxl)
                            .then(if (isCurrent) Modifier.semantics { contentDescription = "Now playing, $number" } else Modifier),
                        contentAlignment = Alignment.CenterEnd,
                    ) {
                        if (isCurrent) {
                            PulseDot(accent, size = 5.dp, active = LocalIsPlaying.current)
                        } else {
                            Text(
                                number,
                                color = Orbit.TextDim,
                                style = OrbitType.Numeric,
                                textAlign = TextAlign.End,
                                maxLines = 1,
                            )
                        }
                    }
                }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                    Text(
                        track.title,
                        color = accent,
                        style = if (isCurrent) OrbitType.RowTitleActive else OrbitType.RowTitle,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        listOf(track.artist, track.qualityLabel).filter { it.isNotBlank() }.joinToString(" · "),
                        color = Orbit.TextDim,
                        style = OrbitType.RowMeta,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (isLiked) Icon(OrbitIcons.Heartfilled, "Favorite", tint = Orbit.Danger, modifier = Modifier.size(Dimens.IconXs))
                Text(formatDuration(track.durationSecs), color = Orbit.TextDim, style = OrbitType.Numeric, maxLines = 1)
                Box(
                    Modifier.size(Dimens.Touch).clickable(onClick = onMore),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(OrbitIcons.More, "More", tint = Orbit.TextDim, modifier = Modifier.size(Dimens.IconM))
                }
            }
        }
        Divider(Modifier.padding(horizontal = Space.Gutter))
    }
}

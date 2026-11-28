package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.draw.rotate
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.withAlpha
import uniffi.rustygrade_ffi.AlbumSummary
import uniffi.rustygrade_ffi.ArtistInfo

/**
 * Album grid card: framed square cover with the desktop corner ticks, an `ALB` badge, title,
 * artist and `year · N TRACKS`. The playing album gets the accent frame, a pulsing dot and the
 * accent rule under its cover. In select mode the card shows a gold border and a check mark
 * while [selected].
 */
@Composable
fun AlbumCard(
    album: AlbumSummary,
    selecting: Boolean,
    selected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val borderStroke = if (selected) BorderStroke(Dimens.Rule, Orbit.Gold) else BorderStroke(Dimens.Stroke, Orbit.LineSoft)
    Column(
        modifier
            .fillMaxWidth()
            .pressable(onLongClick = onLongClick, onClick = onClick)
            .background(Orbit.Card)
            .border(borderStroke),
    ) {
        Box(Modifier.fillMaxWidth().aspectRatio(1f)) {
            Cover(album.thumbPath, Modifier.fillMaxSize(), bordered = false)
            CornerBrackets(Modifier.matchParentSize().padding(Space.S), topLeft = Orbit.Orange, bottomRight = null)
            if (selecting) {
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(Space.S)
                        .size(Dimens.IconL)
                        .background(if (selected) Orbit.Gold else Orbit.Void.withAlpha(Alpha.BackdropTop))
                        .border(BorderStroke(Dimens.Stroke, if (selected) Orbit.Gold else Orbit.TextDim)),
                    contentAlignment = Alignment.Center,
                ) {
                    if (selected) Icon(OrbitIcons.Checksquare, "Selected", tint = Orbit.Void, modifier = Modifier.size(Dimens.IconS))
                }
            }
        }
        Divider()
        Column(
            Modifier.padding(horizontal = Dimens.RowPadV, vertical = 9.dp),
            verticalArrangement = Arrangement.spacedBy(Space.Hair),
        ) {
            Text(
                album.name,
                color = if (selected) Orbit.Gold else Orbit.Text,
                style = OrbitType.CardTitle,
                minLines = 2,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                album.artist,
                color = Orbit.TextDim,
                style = OrbitType.CardMeta,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            val tracks = pluralCount(album.trackCount, "TRACK", "TRACKS")
            Text(
                if (album.year > 0) "${album.year} · $tracks" else tracks,
                color = Orbit.TextMute,
                style = OrbitType.CardBadge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/** One artist in the artists list: square cover, name and `N ALBUMS · M TRACKS`. */
@Composable
fun ArtistRow(artist: ArtistInfo, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 64.dp)
                .pressable(pressedScale = 0.985f, onClick = onClick)
                .padding(horizontal = Space.Gutter, vertical = Dimens.RowPadV),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Dimens.RowGap),
        ) {
            Cover(artist.thumbPath, size = Dimens.SheetCover)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                Text(
                    artist.name,
                    color = Orbit.Text,
                    style = OrbitType.ArtistName,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    "${pluralCount(artist.albumCount, "ALBUM", "ALBUMS")} · ${pluralCount(artist.trackCount, "TRACK", "TRACKS")}",
                    color = Orbit.TextDim,
                    style = OrbitType.ArtistMeta,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Icon(OrbitIcons.Chevrondown, null, tint = Orbit.TextMute, modifier = Modifier.size(Dimens.IconS).rotate(-90f))
        }
        Divider(Modifier.padding(horizontal = Space.Gutter))
    }
}

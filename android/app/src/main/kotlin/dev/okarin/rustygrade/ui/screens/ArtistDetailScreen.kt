package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.QualityPill
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.SectionTitle
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.enterOnce
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.motion.rememberEnterScope
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import kotlinx.coroutines.launch
import uniffi.rustygrade_ffi.ArtistDetail

/**
 * Artist page: the shared [DetailHero] (portrait, name, totals), PLAY ALL / SHUFFLE over every
 * track, and one row per album (newest first, as delivered by the engine).
 */
@Composable
fun ArtistDetailScreen(name: String) {
    val env = LocalEnv.current
    val scope = rememberCoroutineScope()
    val snapshot by env.library.library.collectAsStateWithLifecycle()
    val enter = rememberEnterScope()
    val load by produceState<DetailLoad<ArtistDetail>?>(null, name, snapshot) {
        value = DetailLoad(io { runCatching { env.engine.getArtistDetail(name) }.getOrNull() })
    }
    val detail = load?.value
    if (detail == null) {
        Column(Modifier.fillMaxSize()) {
            ScreenHeader("ARTIST", onBack = { env.nav.pop() })
            if (load != null) EmptyMessage("ARTIST NOT FOUND")
        }
        return
    }

    val artist = detail.artist
    val albums = detail.albums

    /** Collects every track id of the artist off the main thread, then hands them to [then]. */
    fun withAllTrackIds(then: (List<String>) -> Unit) {
        scope.launch {
            val ids = io {
                runCatching {
                    albums.flatMap { env.engine.getAlbumDetail(it.key)?.tracks.orEmpty() }.map { it.id }
                }.getOrDefault(emptyList())
            }
            if (ids.isNotEmpty()) then(ids)
        }
    }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = Space.L)) {
        item(key = "header", contentType = "header") {
            DetailHero(
                coverPath = artist.thumbPath,
                title = artist.name,
                meta = "${pluralCount(artist.albumCount, "ALBUM", "ALBUMS")} · ${pluralCount(artist.trackCount, "TRACK", "TRACKS")}",
                coverOverlay = {
                    // No portrait: a person glyph instead of the album disc the hero would draw.
                    if (artist.thumbPath == null) {
                        Box(Modifier.matchParentSize().background(Orbit.Card), contentAlignment = Alignment.Center) {
                            Icon(OrbitIcons.User, null, tint = Orbit.TextMute, modifier = Modifier.fillMaxSize(0.4f))
                            CornerBrackets(Modifier.matchParentSize().padding(Space.S))
                        }
                    }
                },
                header = { ScreenHeader("ARTIST", onBack = { env.nav.pop() }) },
                actions = {
                    DetailActionButtons(
                        onPlay = { withAllTrackIds { env.playback.playTracks(it, null) } },
                        onShuffle = { withAllTrackIds { env.playback.playTracksShuffled(it) } },
                        enabled = albums.isNotEmpty(),
                        playLabel = "PLAY ALL",
                    )
                },
            )
        }
        if (albums.isEmpty()) {
            item(key = "empty", contentType = "empty") { EmptyMessage("NO ALBUMS") }
        } else {
            item(key = "albums-title", contentType = "section") {
                SectionTitle("ALBUMS", Modifier.padding(start = Space.Gutter, end = Space.Gutter, top = Space.M, bottom = Space.Compact))
            }
        }
        items(albums, key = { it.key }, contentType = { "album" }) { album ->
            Column(Modifier.fillMaxWidth().enterOnce(enter, album.key)) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .pressable(pressedScale = 0.985f) { env.nav.push(Route.Album(album.key)) }
                        .padding(horizontal = Space.Gutter, vertical = Dimens.RowPadV),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.RowGap),
                ) {
                    Box(Modifier.size(64.dp).border(BorderStroke(Dimens.Stroke, Orbit.Line))) {
                        Cover(album.thumbPath, Modifier.fillMaxSize(), bordered = false)
                        CornerBrackets(Modifier.matchParentSize().padding(Space.Xs), length = Space.M)
                    }
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Xs)) {
                        Text(
                            album.name,
                            color = Orbit.Text,
                            style = OrbitType.RowTitle,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            buildList {
                                if (album.year > 0) add(album.year.toString())
                                add(pluralCount(album.trackCount, "TRACK", "TRACKS"))
                            }.joinToString(" · "),
                            color = Orbit.TextDim,
                            style = OrbitType.Badge,
                        )
                        val quality = album.qualityLabel
                        if (quality != null) {
                            QualityPill(quality)
                        }
                    }
                    Icon(
                        OrbitIcons.Chevrondown,
                        null,
                        tint = Orbit.TextMute,
                        modifier = Modifier.size(Dimens.IconS).rotate(-90f),
                    )
                }
                Divider(Modifier.padding(horizontal = Space.Gutter))
            }
        }
    }
}

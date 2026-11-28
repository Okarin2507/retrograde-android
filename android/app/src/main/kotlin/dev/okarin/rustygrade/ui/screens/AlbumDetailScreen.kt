package dev.okarin.rustygrade.ui.screens

import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import kotlinx.coroutines.launch
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.ActionRow
import dev.okarin.rustygrade.ui.components.ConfirmDialog
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.CoverPickerSheet
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.IconTap
import dev.okarin.rustygrade.ui.components.PlaylistPicker
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.SectionTitle
import dev.okarin.rustygrade.ui.components.SheetAction
import dev.okarin.rustygrade.ui.components.TrackActionSheet
import dev.okarin.rustygrade.ui.components.TrackRow
import dev.okarin.rustygrade.ui.components.formatDuration
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.nav.TagTarget
import dev.okarin.rustygrade.ui.theme.Orbit
import uniffi.rustygrade_ffi.AlbumDetail
import uniffi.rustygrade_ffi.TrackInfo

/** One entry of the album list below the header: a disc heading or a track. */
private sealed interface AlbumListRow {
    data class Disc(val number: Int) : AlbumListRow
    data class Track(val index: Int, val track: TrackInfo) : AlbumListRow
}

/** Groups tracks by disc; `index` stays the position in the full album list. */
private fun albumRows(tracks: List<TrackInfo>): List<AlbumListRow> {
    val byDisc = tracks.withIndex().groupBy { it.value.discNumber }.toSortedMap()
    val rows = ArrayList<AlbumListRow>(tracks.size + byDisc.size)
    for ((disc, group) in byDisc) {
        if (byDisc.size > 1) rows += AlbumListRow.Disc(disc)
        for (item in group) rows += AlbumListRow.Track(item.index, item.value)
    }
    return rows
}

/**
 * Album page: cover over a palette-tinted gradient, play/shuffle buttons and the track list
 * (grouped by disc for multi-disc albums). The whole page scrolls as one list.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AlbumDetailScreen(key: String) {
    val env = LocalEnv.current
    val context = LocalContext.current
    val snapshot by env.library.library.collectAsStateWithLifecycle()
    val load by produceState<DetailLoad<AlbumDetail>?>(null, key, snapshot) {
        value = DetailLoad(io { runCatching { env.engine.getAlbumDetail(key) }.getOrNull() })
    }
    val detail = load?.value
    if (detail == null) {
        Column(Modifier.fillMaxSize()) {
            ScreenHeader("ALBUM", onBack = { env.nav.pop() })
            if (load != null) EmptyMessage("ALBUM NOT FOUND")
        }
        return
    }

    val album = detail.album
    val tracks = detail.tracks
    val trackIds = remember(tracks) { tracks.map { it.id } }
    val rows = remember(tracks) { albumRows(tracks) }
    var sheetTrack by remember { mutableStateOf<TrackInfo?>(null) }
    var albumMenu by remember { mutableStateOf(false) }
    var pickingPlaylist by remember { mutableStateOf(false) }
    var showCoverPicker by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    fun say(text: String) = Toast.makeText(context, text, Toast.LENGTH_SHORT).show()

    val coverPath = album.fullPath ?: album.thumbPath
    val meta = buildList {
        if (album.year > 0) add(album.year.toString())
        add(pluralCount(album.trackCount, "TRACK", "TRACKS"))
        add(formatDuration(album.durationSecs))
    }.joinToString(" · ")

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = Space.L)) {
        item(key = "header", contentType = "header") {
            DetailHero(
                coverPath = coverPath,
                title = album.name,
                meta = meta,
                subtitle = album.artist,
                onSubtitle = { env.nav.push(Route.Artist(album.artist)) },
                quality = album.qualityLabel,
                header = {
                    ScreenHeader("ALBUM", onBack = { env.nav.pop() }) {
                        IconTap(OrbitIcons.More, "Album actions", Orbit.Text, iconSize = Dimens.IconM) {
                            pickingPlaylist = false
                            albumMenu = true
                        }
                    }
                },
                actions = {
                    DetailActionButtons(
                        onPlay = { env.playback.playAlbum(key, null) },
                        onShuffle = { env.playback.playAlbumShuffled(key) },
                        enabled = tracks.isNotEmpty(),
                    )
                },
            )
        }
        if (tracks.isEmpty()) {
            item(key = "empty", contentType = "empty") { EmptyMessage("NO TRACKS") }
        }
        items(
            rows,
            key = { row ->
                when (row) {
                    is AlbumListRow.Disc -> "disc-${row.number}"
                    is AlbumListRow.Track -> "track-${row.index}-${row.track.id}"
                }
            },
            contentType = { row ->
                when (row) {
                    is AlbumListRow.Disc -> "disc"
                    is AlbumListRow.Track -> "track"
                }
            },
        ) { row ->
            when (row) {
                is AlbumListRow.Disc -> SectionTitle("DISC ${row.number}", Modifier.padding(start = Space.L, end = Space.L, top = 18.dp, bottom = Space.Compact))
                is AlbumListRow.Track -> TrackRow(
                    row.track,
                    onClick = { env.playback.playAlbum(key, row.index) },
                    onMore = { sheetTrack = row.track },
                    number = row.track.trackNumber.toString().padStart(2, '0'),
                )
            }
        }
    }

    sheetTrack?.let { TrackActionSheet(it, onDismiss = { sheetTrack = null }) }

    if (albumMenu) {
        ModalBottomSheet(
            onDismissRequest = { albumMenu = false },
            shape = RectangleShape,
            containerColor = Orbit.PanelDeep,
            contentColor = Orbit.Text,
            dragHandle = { BottomSheetDefaults.DragHandle(color = Orbit.Line) },
        ) {
            Column(Modifier.navigationBarsPadding().padding(bottom = Space.M)) {
                Row(
                    Modifier.padding(horizontal = Space.L, vertical = Dimens.RowPadV),
                    horizontalArrangement = Arrangement.spacedBy(Space.M),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Cover(coverPath, size = Dimens.SheetCover)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                        Text(album.name, color = Orbit.Text, style = OrbitType.SheetTitle, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(album.artist, color = Orbit.TextDim, style = OrbitType.RowMeta, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
                Divider()
                if (pickingPlaylist) {
                    PlaylistPicker(trackIds = trackIds) { name ->
                        say("ADDED ALBUM TO $name")
                        albumMenu = false
                    }
                } else {
                    val actions = listOf(
                        SheetAction("PLAY NEXT", OrbitIcons.Playnext, enabled = trackIds.isNotEmpty()) {
                            env.playback.playNext(trackIds)
                            say("PLAYS NEXT")
                            albumMenu = false
                        },
                        SheetAction("ADD TO QUEUE", OrbitIcons.Listmusic, enabled = trackIds.isNotEmpty()) {
                            env.playback.enqueue(trackIds)
                            say("ADDED TO QUEUE")
                            albumMenu = false
                        },
                        SheetAction("ADD TO PLAYLIST", OrbitIcons.Plus, enabled = trackIds.isNotEmpty()) {
                            pickingPlaylist = true
                        },
                        SheetAction("EDIT ALBUM TAGS", OrbitIcons.Pencil) {
                            albumMenu = false
                            env.nav.push(Route.TagEditor(TagTarget.Album(key)))
                        },
                        SheetAction("FETCH COVER ONLINE", OrbitIcons.Download) {
                            albumMenu = false
                            showCoverPicker = true
                        },
                        SheetAction("SCAN REPLAYGAIN", OrbitIcons.Sliders, enabled = trackIds.isNotEmpty()) {
                            albumMenu = false
                            say("SCANNING REPLAYGAIN…")
                            env.library.scanAlbumReplayGain(key) { msg -> say(msg) }
                        },
                        SheetAction("REMOVE FROM LIBRARY", OrbitIcons.Trash, danger = true) {
                            albumMenu = false
                            confirmDelete = true
                        },
                    )
                    Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                        actions.forEach { ActionRow(it) }
                    }
                }
            }
        }
    }

    if (showCoverPicker) {
        CoverPickerSheet(
            initialAlbum = album.name,
            initialArtist = album.artist,
            trackTitle = tracks.firstOrNull()?.title.orEmpty(),
            year = album.year,
            onDismiss = { showCoverPicker = false },
            onPickedFile = { file ->
                scope.launch {
                    val ok = io {
                        try {
                            env.engine.applyAlbumCover(key, file.absolutePath) != null
                        } finally {
                            file.delete()
                        }
                    }
                    if (ok) {
                        env.library.reload()
                        say("COVER APPLIED")
                    } else {
                        say("COULD NOT APPLY COVER")
                    }
                }
            },
        )
    }

    if (confirmDelete) {
        ConfirmDialog(
            message = "REMOVE \"${album.name}\" FROM THE LIBRARY? THE MUSIC FILES ARE NOT DELETED.",
            confirmLabel = "DELETE",
            onConfirm = {
                confirmDelete = false
                env.library.deleteAlbums(listOf(key)) { removed ->
                    if (removed > 0) env.nav.pop()
                }
            },
            onDismiss = { confirmDelete = false },
        )
    }
}

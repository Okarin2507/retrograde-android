package dev.okarin.rustygrade.ui.screens

import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.PlaylistIoUi
import dev.okarin.rustygrade.media.CoverProvider
import dev.okarin.rustygrade.storage.PlaylistFiles
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.ConfirmDialog
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.SheetAction
import dev.okarin.rustygrade.ui.components.TrackActionSheet
import dev.okarin.rustygrade.ui.components.TrackRow
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.nav.PlaylistKind
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.player.dragReorder
import dev.okarin.rustygrade.ui.player.rememberDragReorderState
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import kotlin.coroutines.cancellation.CancellationException
import kotlin.math.roundToInt
import kotlinx.coroutines.launch
import uniffi.rustygrade_ffi.TrackInfo

/**
 * Favorites, a custom playlist, or a smart playlist: header block with PLAY / SHUFFLE, an overflow
 * menu (rename, cover, edit, delete) and the track list. Reloads whenever the collections change.
 */
@Composable
fun PlaylistDetailScreen(kind: PlaylistKind) {
    val env = LocalEnv.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snapshot by env.collections.collections.collectAsStateWithLifecycle()

    val libraryVersion by env.library.library.collectAsStateWithLifecycle()
    val custom = (kind as? PlaylistKind.Custom)?.let { k -> snapshot?.playlists?.firstOrNull { it.id == k.id } }
    val smart = (kind as? PlaylistKind.Smart)?.let { k -> snapshot?.smart?.firstOrNull { it.id == k.id } }
    val loaded = snapshot != null
    val missing = loaded && kind !is PlaylistKind.Favorites && custom == null && smart == null

    // What the track list depends on; a change reloads it.
    val sourceKey: Any? = when (kind) {
        PlaylistKind.Favorites -> snapshot?.liked
        is PlaylistKind.Custom -> custom?.trackIds
        is PlaylistKind.Smart -> Triple(smart, snapshot?.liked, libraryVersion)
    }

    var tracks by remember(kind) { mutableStateOf<List<TrackInfo>?>(null) }
    LaunchedEffect(kind, loaded, sourceKey) {
        if (!loaded) return@LaunchedEffect
        val loadedTracks = try {
            io {
                when (kind) {
                    PlaylistKind.Favorites -> env.engine.getTracks(snapshot?.liked.orEmpty())
                    is PlaylistKind.Custom -> env.engine.getTracks(custom?.trackIds.orEmpty())
                    is PlaylistKind.Smart -> env.engine.smartPlaylistTracks(kind.id)
                }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            emptyList()
        }
        tracks = loadedTracks
    }

    val title = when (kind) {
        PlaylistKind.Favorites -> "Favorites"
        is PlaylistKind.Custom -> custom?.name ?: "PLAYLIST"
        is PlaylistKind.Smart -> smart?.name ?: "SMART PLAYLIST"
    }
    // The pinned bar names the kind; the playlist's own name sits in the hero below it.
    val headerTitle = when (kind) {
        PlaylistKind.Favorites -> "FAVORITES"
        is PlaylistKind.Custom -> "PLAYLIST"
        is PlaylistKind.Smart -> "SMART PLAYLIST"
    }

    var menuOpen by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var reordering by remember(kind) { mutableStateOf(false) }
    var sheetTrack by remember { mutableStateOf<TrackInfo?>(null) }
    val ioUi by env.collections.ioUi.collectAsStateWithLifecycle()

    val list = tracks
    val ids = remember(list) { list.orEmpty().map { it.id } }
    val canReorder = (kind is PlaylistKind.Custom || kind is PlaylistKind.Favorites) && ids.size > 1
    // Derived, not written back during composition; the flag itself is reset in an effect.
    val activeReordering = reordering && canReorder
    LaunchedEffect(canReorder) { if (!canReorder) reordering = false }
    val totalSecs = remember(list) { list?.sumOf { it.durationSecs } ?: 0.0 }
    val rawIds = remember(kind, custom, snapshot) {
        when (kind) {
            PlaylistKind.Favorites -> snapshot?.liked.orEmpty()
            is PlaylistKind.Custom -> custom?.trackIds.orEmpty()
            is PlaylistKind.Smart -> emptyList()
        }
    }

    fun commitMove(fromVisible: Int, toVisible: Int) {
        val current = tracks ?: return
        val visibleIds = current.map { it.id }
        val pair = resolveRawMoveIndices(rawIds, visibleIds, fromVisible, toVisible) ?: return
        val updated = current.toMutableList()
        updated.add(toVisible, updated.removeAt(fromVisible))
        tracks = updated
        when (kind) {
            is PlaylistKind.Custom -> env.collections.movePlaylistTrack(kind.id, pair.first, pair.second)
            PlaylistKind.Favorites -> env.collections.moveLikedTrack(pair.first, pair.second)
            is PlaylistKind.Smart -> Unit
        }
    }

    val listState = rememberLazyListState()
    val drag = rememberDragReorderState(
        listState = listState,
        baseIndex = { 0 },
        itemIndexOffset = 1,
        canDragKey = { it != "header" && it != "empty" },
        onCommit = { from, to -> commitMove(from, to) },
    )
    LaunchedEffect(sourceKey) {
        if (drag.isDragging) drag.cancel()
    }

    // Cover picker (custom playlists only) and export launchers. Their callbacks outlive recompositions.
    val currentEnv by rememberUpdatedState(env)
    val currentKind by rememberUpdatedState(kind)
    val customId by rememberUpdatedState((kind as? PlaylistKind.Custom)?.id)
    val exportM3uLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("audio/x-mpegurl"),
    ) { uri: Uri? ->
        if (uri != null) {
            currentEnv.collections.exportPlaylistM3u8(currentKind, uri)
        }
    }
    val exportZipLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("application/zip"),
    ) { uri: Uri? ->
        val id = customId
        if (uri != null && id != null) {
            currentEnv.collections.exportPlaylistZip(id, uri)
        }
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri: Uri? ->
        val id = customId
        if (uri != null && id != null) {
            scope.launch {
                val path = try {
                    io {
                        CoverProvider.copyPickedImage(context, uri, context.cacheDir, "playlist-cover-$id-")
                            .absolutePath
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toast.makeText(context, e.message ?: "Could not read the image", Toast.LENGTH_SHORT).show()
                    null
                }
                if (path != null) {
                    env.collections.setPlaylistCover(id, path, deleteSourceAfter = true) { err ->
                        if (err != null) Toast.makeText(context, err, Toast.LENGTH_SHORT).show()
                    }
                }
            }
        }
    }
    fun pickCover() = picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))

    Column(Modifier.fillMaxSize()) {
        // Pinned rather than inside the hero, so DONE stays reachable while reordering a long list.
        // While reordering the hero is usually scrolled away: name the playlist in the bar then.
        ScreenHeader(if (activeReordering) title else headerTitle, onBack = { env.nav.pop() }) {
            if (loaded && !missing) {
                if (activeReordering) {
                    OrbitButton(
                        "DONE",
                        onClick = { reordering = false },
                        modifier = Modifier.padding(end = Space.Xs),
                        primary = true,
                    )
                }
                Box {
                    Box(
                        Modifier.size(Dimens.Touch).pressable(pressedScale = 0.9f) { menuOpen = true },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(OrbitIcons.More, "More", tint = Orbit.Text, modifier = Modifier.size(Dimens.IconM))
                    }
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }, containerColor = Orbit.Raised) {
                        @Composable
                        fun item(label: String, icon: ImageVector? = null, danger: Boolean = false, onClick: () -> Unit) {
                            DropdownMenuItem(
                                leadingIcon = icon?.let { ic ->
                                    { Icon(ic, null, tint = if (danger) Orbit.Danger else Orbit.Text, modifier = Modifier.size(Dimens.IconS)) }
                                },
                                text = { Text(label, color = if (danger) Orbit.Danger else Orbit.Text, style = OrbitType.Hud) },
                                onClick = { menuOpen = false; onClick() },
                            )
                        }
                        if (ids.isNotEmpty()) {
                            item("PLAY NEXT", icon = OrbitIcons.Playnext) {
                                env.playback.playNext(ids)
                                Toast.makeText(context, "PLAYS NEXT", Toast.LENGTH_SHORT).show()
                            }
                            item("ADD TO QUEUE", icon = OrbitIcons.Listmusic) {
                                env.playback.enqueue(ids)
                                Toast.makeText(context, "ADDED TO QUEUE", Toast.LENGTH_SHORT).show()
                            }
                        }
                        if (canReorder) {
                            item(if (activeReordering) "DONE REORDERING" else "REORDER TRACKS", icon = OrbitIcons.Grip) {
                                reordering = !activeReordering
                            }
                        }
                        when (kind) {
                            PlaylistKind.Favorites -> {
                                item("EXPORT M3U8", icon = OrbitIcons.Download) {
                                    exportM3uLauncher.launch(PlaylistFiles.sanitizeExportFilename(title, "m3u8"))
                                }
                            }
                            is PlaylistKind.Custom -> {
                                item("RENAME") { renaming = true }
                                item("PICK COVER") { pickCover() }
                                if (custom?.coverPath != null) {
                                    item("REMOVE COVER") {
                                        env.collections.setPlaylistCover(kind.id, null) { err ->
                                            if (err != null) Toast.makeText(context, err, Toast.LENGTH_SHORT).show()
                                        }
                                    }
                                }
                                item("EXPORT M3U8", icon = OrbitIcons.Download) {
                                    exportM3uLauncher.launch(PlaylistFiles.sanitizeExportFilename(title, "m3u8"))
                                }
                                item("EXPORT ZIP", icon = OrbitIcons.Download) {
                                    exportZipLauncher.launch(PlaylistFiles.sanitizeExportFilename(title, "zip"))
                                }
                                item("DELETE", danger = true) { confirmDelete = true }
                            }
                            is PlaylistKind.Smart -> {
                                item("EDIT") { env.nav.push(Route.SmartEditor(kind.id)) }
                                item("EXPORT M3U8", icon = OrbitIcons.Download) {
                                    exportM3uLauncher.launch(PlaylistFiles.sanitizeExportFilename(title, "m3u8"))
                                }
                                item("DELETE", danger = true) { confirmDelete = true }
                            }
                        }
                    }
                }
            }
        }
        when (val status = ioUi) {
            PlaylistIoUi.Idle -> Unit
            is PlaylistIoUi.Busy -> IoStatusBanner(status.message, Orbit.Gold, onDismiss = null)
            is PlaylistIoUi.Done -> IoStatusBanner(status.message, Orbit.Teal, onDismiss = { env.collections.clearIoStatus() })
            is PlaylistIoUi.Failed -> IoStatusBanner(status.message, Orbit.Danger, onDismiss = { env.collections.clearIoStatus() })
        }

        when {
            missing -> EmptyMessage("PLAYLIST NOT FOUND")
            list == null -> EmptyMessage("LOADING…")
            else -> Box(Modifier.weight(1f).fillMaxWidth()) {
                // Only an active drag draws the accent (insertion marker, ghost border).
                val insertionAccent = if (drag.isDragging) rememberReadableAccentOn(Orbit.Void) else Orbit.Line
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize().dragReorder(drag, enabled = canReorder),
                    contentPadding = PaddingValues(bottom = Space.L),
                ) {
                    item(key = "header", contentType = "header") {
                        val coverPath = custom?.coverPath
                        DetailHero(
                            coverPath = coverPath,
                            title = title,
                            meta = "${pluralCount(list.size, "TRACK", "TRACKS")} · ${totalLabel(totalSecs)}",
                            coverOverlay = {
                                when (kind) {
                                    PlaylistKind.Favorites -> HeroIconTile(OrbitIcons.Heartfilled, Orbit.Danger)
                                    is PlaylistKind.Smart -> HeroIconTile(OrbitIcons.Filter, Orbit.Gold)
                                    is PlaylistKind.Custom -> Box(
                                        Modifier
                                            .matchParentSize()
                                            .clickable(
                                                onClickLabel = if (coverPath == null) "Choose cover" else "Change cover",
                                                role = Role.Button,
                                                onClick = ::pickCover,
                                            )
                                            .semantics { contentDescription = "Playlist cover" },
                                    )
                                }
                            },
                            header = {},
                            actions = {
                                Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                                    if (kind is PlaylistKind.Custom) {
                                        Text(
                                            if (coverPath == null) "TAP COVER TO CHOOSE AN IMAGE" else "TAP COVER TO CHANGE",
                                            modifier = Modifier.padding(bottom = Space.M),
                                            color = Orbit.TextMute,
                                            style = OrbitType.Kicker,
                                        )
                                    }
                                    DetailActionButtons(
                                        onPlay = { if (ids.isNotEmpty()) env.playback.playTracks(ids, null) },
                                        onShuffle = { if (ids.isNotEmpty()) env.playback.playTracksShuffled(ids) },
                                        enabled = list.isNotEmpty(),
                                    )
                                }
                            },
                        )
                    }
                    if (list.isEmpty()) {
                        item(key = "empty", contentType = "empty") {
                            EmptyMessage(
                                when (kind) {
                                    PlaylistKind.Favorites -> "NO FAVORITES YET · TAP THE HEART ON A TRACK"
                                    is PlaylistKind.Custom -> "EMPTY · ADD TRACKS FROM A TRACK'S ⋮ MENU"
                                    is PlaylistKind.Smart -> "NO TRACKS MATCH THESE RULES"
                                },
                            )
                        }
                    } else {
                        itemsIndexed(list, key = { index, track -> "$index:${track.id}" }, contentType = { _, _ -> "track" }) { index, track ->
                            val rowKey = "$index:${track.id}"
                            val dragging = drag.isDragging
                            val indicator = when {
                                !dragging || index != drag.targetIndex || index == drag.fromIndex -> 0
                                drag.targetIndex < drag.fromIndex -> -1
                                else -> 1
                            }
                            val placeholder = dragging && drag.draggedKey == rowKey
                            Row(
                                Modifier
                                    .fillMaxWidth()
                                    .alpha(if (placeholder) Alpha.Faint else 1f)
                                    .drawBehind {
                                        val bar = Dimens.Rule.toPx()
                                        if (indicator < 0) drawRect(insertionAccent, size = Size(size.width, bar))
                                        else if (indicator > 0) drawRect(insertionAccent, topLeft = Offset(0f, size.height - bar), size = Size(size.width, bar))
                                    }
                                    .then(
                                        if (canReorder) {
                                            Modifier.semantics {
                                                customActions = buildList {
                                                    if (index > 0) add(CustomAccessibilityAction("Move up") { commitMove(index, index - 1); true })
                                                    if (index < list.lastIndex) add(CustomAccessibilityAction("Move down") { commitMove(index, index + 1); true })
                                                }
                                            }
                                        } else {
                                            Modifier
                                        },
                                    ),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                TrackRow(
                                    track = track,
                                    onClick = { env.playback.playTracks(ids, index) },
                                    onMore = { sheetTrack = track },
                                    modifier = Modifier.weight(1f),
                                    showCover = true,
                                )
                                if (activeReordering) {
                                    Icon(
                                        OrbitIcons.Grip,
                                        "Hold and drag to reorder",
                                        tint = Orbit.Gold,
                                        modifier = Modifier.padding(end = Space.M).size(Dimens.IconMd),
                                    )
                                }
                            }
                        }
                    }
                }
                if (drag.isDragging) {
                    val ghostTrack = remember(drag.draggedKey, list) { list.getOrNull(drag.fromIndex) }
                    if (ghostTrack != null) {
                        Box(
                            Modifier
                                .fillMaxWidth()
                                .offset { IntOffset(0, drag.ghostTop.roundToInt()) }
                                .background(Orbit.Raised)
                                .border(Dimens.Stroke, insertionAccent),
                        ) {
                            TrackRow(track = ghostTrack, onClick = {}, onMore = {}, showCover = true)
                        }
                    }
                }
            }
        }
    }

    if (renaming && custom != null) {
        NameDialog(
            title = "RENAME PLAYLIST",
            initial = custom.name,
            confirmLabel = "RENAME",
            onConfirm = { name ->
                renaming = false
                env.collections.renamePlaylist(custom.id, name)
            },
            onDismiss = { renaming = false },
        )
    }

    if (confirmDelete) {
        ConfirmDialog(
            message = "DELETE \"$title\"? THIS CANNOT BE UNDONE.",
            confirmLabel = "DELETE",
            onConfirm = {
                confirmDelete = false
                when (kind) {
                    is PlaylistKind.Custom -> env.collections.deletePlaylist(kind.id) { env.nav.pop() }
                    is PlaylistKind.Smart -> env.collections.deleteSmart(kind.id) { env.nav.pop() }
                    PlaylistKind.Favorites -> Unit
                }
            },
            onDismiss = { confirmDelete = false },
        )
    }

    sheetTrack?.let { track ->
        val extra = if (kind is PlaylistKind.Custom) {
            listOf(
                SheetAction("REMOVE FROM PLAYLIST", OrbitIcons.Trash, danger = true) {
                    env.collections.removeFromPlaylist(kind.id, track.id)
                    sheetTrack = null
                },
            )
        } else {
            emptyList()
        }
        TrackActionSheet(track, onDismiss = { sheetTrack = null }, extraActions = extra)
    }
}

/**
 * Opaque stand-in for the hero cover of Favorites and smart playlists: the kind icon on a card
 * tinted with [tint], re-drawing the corner ticks the overlay hides.
 */
@Composable
private fun BoxScope.HeroIconTile(icon: ImageVector, tint: Color) {
    Box(
        Modifier
            .matchParentSize()
            .background(Orbit.Card)
            .background(tint.withAlpha(Alpha.Wash)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = tint, modifier = Modifier.fillMaxSize(0.4f))
        CornerBrackets(Modifier.matchParentSize().padding(Space.S))
    }
}

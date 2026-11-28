package dev.okarin.rustygrade.ui.screens

import android.net.Uri
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.PlaylistIoUi
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.ConfirmDialog
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.Kicker
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.enterOnce
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.motion.rememberEnterScope
import dev.okarin.rustygrade.ui.nav.PlaylistKind
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import kotlinx.coroutines.launch
import uniffi.rustygrade_ffi.PlaylistInfo
import uniffi.rustygrade_ffi.SmartPlaylistInfo

/** Lead tile of a playlist row (cover or icon block). */
private val RowTile = 56.dp

/**
 * The PLAYLISTS tab (`DATA_BANK`, the desktop name of the section): FAVORITES, custom playlists and smart playlists in one scrolling list, each
 * section with a NEW button and an empty-state hint.
 */
@Composable
fun PlaylistsScreen() {
    val env = LocalEnv.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snapshot by env.collections.collections.collectAsStateWithLifecycle()
    val ioUi by env.collections.ioUi.collectAsStateWithLifecycle()
    var naming by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf<PlaylistInfo?>(null) }
    var deletingCustom by remember { mutableStateOf<PlaylistInfo?>(null) }
    var deletingSmart by remember { mutableStateOf<SmartPlaylistInfo?>(null) }

    val currentCollections by rememberUpdatedState(env.collections)
    val importLauncher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri != null) {
            currentCollections.importPlaylistFile(uri)
        }
    }

    fun say(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()

    fun withSmartIds(id: Long, then: (List<String>) -> Unit) {
        scope.launch {
            val ids = io { runCatching { env.engine.smartPlaylistTracks(id).map { it.id } }.getOrDefault(emptyList()) }
            if (ids.isEmpty()) say("NO MATCHING TRACKS") else then(ids)
        }
    }

    val likedIds = snapshot?.liked.orEmpty()
    val likedCount = likedIds.size
    val playlists = snapshot?.playlists.orEmpty()
    val smart = snapshot?.smart.orEmpty()
    val importing = ioUi is PlaylistIoUi.Busy
    val enter = rememberEnterScope()

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("DATA_BANK") {
            OrbitButton(
                label = if (importing) "IMPORTING…" else "IMPORT",
                onClick = { importLauncher.launch(arrayOf("*/*")) },
                modifier = Modifier.padding(end = Space.M),
                icon = OrbitIcons.Upload,
                enabled = !importing,
            )
        }
        when (val status = ioUi) {
            PlaylistIoUi.Idle -> Unit
            is PlaylistIoUi.Busy -> IoStatusBanner(status.message, Orbit.Gold, onDismiss = null)
            is PlaylistIoUi.Done -> IoStatusBanner(status.message, Orbit.Teal, onDismiss = { env.collections.clearIoStatus() })
            is PlaylistIoUi.Failed -> IoStatusBanner(status.message, Orbit.Danger, onDismiss = { env.collections.clearIoStatus() })
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = Space.Xl)) {
            item(key = "favorites", contentType = "playlist") {
                var favMenu by remember { mutableStateOf(false) }
                ListRow(
                    onClick = { env.nav.push(Route.Playlist(PlaylistKind.Favorites)) },
                    modifier = Modifier.enterOnce(enter, "favorites"),
                    lead = { IconBlock(OrbitIcons.Heartfilled, Orbit.Danger, RowTile, ticks = true) },
                    title = "Favorites",
                    subtitle = pluralCount(likedCount, "TRACK", "TRACKS"),
                    menu = {
                        PlaylistOverflow(expanded = favMenu, onOpen = { favMenu = true }, onDismiss = { favMenu = false }) {
                            MenuItem("PLAY", enabled = likedIds.isNotEmpty()) { favMenu = false; env.playback.playTracks(likedIds, null) }
                            MenuItem("SHUFFLE", enabled = likedIds.isNotEmpty()) { favMenu = false; env.playback.playTracksShuffled(likedIds) }
                        }
                    },
                )
            }

            item(key = "head-playlists", contentType = "section") {
                SectionHeader("PLAYLISTS", onNew = { naming = true })
            }
            if (playlists.isEmpty()) {
                item(key = "empty-playlists", contentType = "hint") { Hint("NO PLAYLISTS YET · TAP NEW TO CREATE ONE") }
            } else {
                items(playlists, key = { "pl-${it.id}" }, contentType = { "playlist" }) { playlist ->
                    PlaylistRow(
                        playlist = playlist,
                        modifier = Modifier.enterOnce(enter, "pl-${playlist.id}"),
                        onPlay = { env.playback.playTracks(playlist.trackIds, null) },
                        onShuffle = { env.playback.playTracksShuffled(playlist.trackIds) },
                        onPlayNext = { env.playback.playNext(playlist.trackIds); say("PLAYS NEXT") },
                        onEnqueue = { env.playback.enqueue(playlist.trackIds); say("ADDED TO QUEUE") },
                        onRename = { renaming = playlist },
                        onDelete = { deletingCustom = playlist },
                    )
                }
            }

            item(key = "head-smart", contentType = "section") {
                SectionHeader("SMART PLAYLISTS", onNew = { env.nav.push(Route.SmartEditor(0)) })
            }
            if (smart.isEmpty()) {
                item(key = "empty-smart", contentType = "hint") { Hint("NO SMART PLAYLISTS · RULES BUILD THEM FROM YOUR LIBRARY") }
            } else {
                items(smart, key = { "sm-${it.id}" }, contentType = { "playlist" }) { playlist ->
                    SmartRow(
                        playlist = playlist,
                        modifier = Modifier.enterOnce(enter, "sm-${playlist.id}"),
                        onPlay = { withSmartIds(playlist.id) { env.playback.playTracks(it, null) } },
                        onShuffle = { withSmartIds(playlist.id) { env.playback.playTracksShuffled(it) } },
                        onEdit = { env.nav.push(Route.SmartEditor(playlist.id)) },
                        onDelete = { deletingSmart = playlist },
                    )
                }
            }
        }
    }

    if (naming) {
        NameDialog(
            title = "NEW PLAYLIST",
            initial = "",
            confirmLabel = "CREATE",
            onConfirm = { name ->
                naming = false
                env.collections.createPlaylist(name) { created ->
                    env.nav.push(Route.Playlist(PlaylistKind.Custom(created.id)))
                }
            },
            onDismiss = { naming = false },
        )
    }

    renaming?.let { pl ->
        NameDialog(
            title = "RENAME PLAYLIST",
            initial = pl.name,
            confirmLabel = "RENAME",
            onConfirm = { name ->
                renaming = null
                env.collections.renamePlaylist(pl.id, name)
            },
            onDismiss = { renaming = null },
        )
    }

    deletingCustom?.let { pl ->
        ConfirmDialog(
            message = "DELETE \"${pl.name}\"? THIS CANNOT BE UNDONE.",
            confirmLabel = "DELETE",
            onConfirm = {
                deletingCustom = null
                env.collections.deletePlaylist(pl.id)
            },
            onDismiss = { deletingCustom = null },
        )
    }

    deletingSmart?.let { sm ->
        ConfirmDialog(
            message = "DELETE \"${sm.name}\"? THIS CANNOT BE UNDONE.",
            confirmLabel = "DELETE",
            onConfirm = {
                deletingSmart = null
                env.collections.deleteSmart(sm.id)
            },
            onDismiss = { deletingSmart = null },
        )
    }
}

@Composable
private fun SectionHeader(title: String, onNew: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(start = Space.Gutter, end = Space.Gutter, top = 22.dp, bottom = Dimens.RowPadV),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Kicker(title, Modifier.weight(1f))
        OrbitButton("NEW", onClick = onNew, icon = OrbitIcons.Plus)
    }
}

@Composable
private fun Hint(text: String) {
    Text(
        text,
        modifier = Modifier.fillMaxWidth().padding(horizontal = Space.Gutter, vertical = Space.M),
        color = Orbit.TextDim,
        style = OrbitType.Body,
    )
}

@Composable
private fun PlaylistRow(
    playlist: PlaylistInfo,
    modifier: Modifier = Modifier,
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    onPlayNext: () -> Unit,
    onEnqueue: () -> Unit,
    onRename: () -> Unit,
    onDelete: () -> Unit,
) {
    val env = LocalEnv.current
    var menu by remember { mutableStateOf(false) }
    val hasTracks = playlist.trackIds.isNotEmpty()
    ListRow(
        onClick = { env.nav.push(Route.Playlist(PlaylistKind.Custom(playlist.id))) },
        modifier = modifier,
        lead = { FramedCover(playlist.coverPath, RowTile) },
        title = playlist.name,
        subtitle = pluralCount(playlist.trackIds.size, "TRACK", "TRACKS"),
        menu = {
            PlaylistOverflow(expanded = menu, onOpen = { menu = true }, onDismiss = { menu = false }) {
                MenuItem("PLAY", enabled = hasTracks) { menu = false; onPlay() }
                MenuItem("SHUFFLE", enabled = hasTracks) { menu = false; onShuffle() }
                MenuItem("PLAY NEXT", enabled = hasTracks) { menu = false; onPlayNext() }
                MenuItem("ADD TO QUEUE", enabled = hasTracks) { menu = false; onEnqueue() }
                MenuItem("RENAME") { menu = false; onRename() }
                MenuItem("DELETE", danger = true) { menu = false; onDelete() }
            }
        },
    )
}

@Composable
private fun SmartRow(
    playlist: SmartPlaylistInfo,
    modifier: Modifier = Modifier,
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    onEdit: () -> Unit,
    onDelete: () -> Unit,
) {
    val env = LocalEnv.current
    var menu by remember { mutableStateOf(false) }
    val mode = if (playlist.matchAll) "MATCH ALL" else "MATCH ANY"
    val rules = playlist.rules.size
    ListRow(
        onClick = { env.nav.push(Route.Playlist(PlaylistKind.Smart(playlist.id))) },
        modifier = modifier,
        lead = { IconBlock(OrbitIcons.Filter, Orbit.Gold, RowTile, ticks = true) },
        title = playlist.name,
        subtitle = "$mode · $rules ${if (rules == 1) "RULE" else "RULES"}",
        menu = {
            PlaylistOverflow(expanded = menu, onOpen = { menu = true }, onDismiss = { menu = false }) {
                MenuItem("PLAY") { menu = false; onPlay() }
                MenuItem("SHUFFLE") { menu = false; onShuffle() }
                MenuItem("EDIT") { menu = false; onEdit() }
                MenuItem("DELETE", danger = true) { menu = false; onDelete() }
            }
        },
    )
}

@Composable
private fun ListRow(
    onClick: () -> Unit,
    lead: @Composable () -> Unit,
    title: String,
    subtitle: String,
    menu: @Composable () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .pressable(pressedScale = 0.985f, onClick = onClick)
                .padding(start = Space.Gutter, end = Space.Xs, top = Dimens.RowPadV, bottom = Dimens.RowPadV),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Dimens.RowGap),
        ) {
            lead()
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                Text(
                    title,
                    color = Orbit.Text,
                    style = OrbitType.RowTitle,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    subtitle,
                    color = Orbit.TextDim,
                    style = OrbitType.Badge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            menu()
        }
        Divider(Modifier.padding(horizontal = Space.Gutter))
    }
}

@Composable
private fun PlaylistOverflow(
    expanded: Boolean,
    onOpen: () -> Unit,
    onDismiss: () -> Unit,
    content: @Composable () -> Unit,
) {
    Box {
        Box(
            Modifier.size(Dimens.Touch).pressable(pressedScale = 0.9f, onClick = onOpen),
            contentAlignment = Alignment.Center,
        ) {
            Icon(OrbitIcons.More, "Playlist actions", tint = Orbit.TextDim, modifier = Modifier.size(Dimens.IconMd))
        }
        DropdownMenu(expanded = expanded, onDismissRequest = onDismiss, containerColor = Orbit.Raised) {
            content()
        }
    }
}

@Composable
private fun MenuItem(label: String, enabled: Boolean = true, danger: Boolean = false, onClick: () -> Unit) {
    DropdownMenuItem(
        text = {
            Text(
                label,
                color = when {
                    !enabled -> Orbit.TextMute
                    danger -> Orbit.Danger
                    else -> Orbit.Text
                },
                style = OrbitType.Hud,
            )
        },
        enabled = enabled,
        onClick = onClick,
    )
}

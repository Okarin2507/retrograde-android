package dev.okarin.rustygrade.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.draw.clip
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.ScanUi
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.ConfirmDialog
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.IconTap
import dev.okarin.rustygrade.ui.motion.StatusDot
import dev.okarin.rustygrade.ui.motion.enterOnce
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.motion.rememberEnterScope
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha
import uniffi.rustygrade_ffi.LibrarySnapshot
import uniffi.rustygrade_ffi.SortOption
import uniffi.rustygrade_ffi.librarySorts

/**
 * The LIBRARY tab: albums grid or artists list, sort menu, multi-select album removal and
 * scan feedback.
 */
@Composable
fun LibraryScreen() {
    val env = LocalEnv.current
    val snapshot by env.library.library.collectAsStateWithLifecycle()
    val settings by env.library.settings.collectAsStateWithLifecycle()
    val scan by env.library.scan.collectAsStateWithLifecycle()

    // Local override so the chip reacts immediately; the persisted setting is the source of truth otherwise.
    var viewOverride by rememberSaveable { mutableStateOf<String?>(null) }
    LaunchedEffect(settings?.libraryView) {
        if (settings?.libraryView == viewOverride) viewOverride = null
    }
    val view = viewOverride ?: settings?.libraryView ?: "albums"
    val albumsView = view != "artists"

    // Hoisted at LibraryScreen scope (inside SaveableStateProvider("tab:Library")) so Back from
    // Album/Artist restores the exact offset, while Home tap, view switch, and sort change reset it.
    val albumGridState = rememberLazyGridState()
    val artistListState = rememberLazyListState()

    LaunchedEffect(env.nav.libraryScrollToTopPending) {
        if (env.nav.consumeLibraryScrollToTop()) {
            albumGridState.scrollToItem(0)
            artistListState.scrollToItem(0)
        }
    }
    var lastAlbumsView by rememberSaveable { mutableStateOf(albumsView) }
    LaunchedEffect(albumsView) {
        if (lastAlbumsView != albumsView) {
            lastAlbumsView = albumsView
            albumGridState.scrollToItem(0)
            artistListState.scrollToItem(0)
        }
    }
    var lastSort by rememberSaveable { mutableStateOf(snapshot?.sort) }
    LaunchedEffect(snapshot?.sort) {
        val currentSort = snapshot?.sort ?: return@LaunchedEffect
        val previous = lastSort
        lastSort = currentSort
        if (previous != null && previous != currentSort) {
            albumGridState.scrollToItem(0)
            artistListState.scrollToItem(0)
        }
    }

    var selecting by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf(setOf<String>()) }
    var confirmDelete by remember { mutableStateOf(false) }

    fun exitSelect() {
        selecting = false
        selected = emptySet()
        confirmDelete = false
    }

    BackHandler(enabled = selecting && !env.nav.playerOpen) { exitSelect() }

    val sorts by produceState<List<SortOption>>(emptyList()) { value = io { librarySorts() } }
    var sortMenu by remember { mutableStateOf(false) }
    val scanningNow = scan is ScanUi.Running

    Column(Modifier.fillMaxSize()) {
        TerminalHeader {
            Box(
                Modifier
                    .size(Dimens.Touch)
                    .clickable(enabled = !scanningNow, role = Role.Button) { env.scan() },
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    OrbitIcons.Refresh,
                    "Rescan library",
                    tint = if (scanningNow) Orbit.TextMute else Orbit.Text,
                    modifier = Modifier.size(Dimens.IconM),
                )
            }
            Box {
                IconTap(OrbitIcons.Arrowupdown, "Sort", Orbit.Text, iconSize = Dimens.IconM) { sortMenu = true }
                DropdownMenu(
                    expanded = sortMenu,
                    onDismissRequest = { sortMenu = false },
                    containerColor = Orbit.Raised,
                ) {
                    val current = snapshot?.sort
                    sorts.forEach { option ->
                        DropdownMenuItem(
                            text = {
                                Text(
                                    option.label.uppercase(),
                                    color = if (option.key == current) Orbit.Gold else Orbit.Text,
                                    style = if (option.key == current) OrbitType.NumericBold else OrbitType.Numeric,
                                )
                            },
                            onClick = {
                                sortMenu = false
                                env.library.setSort(option.key)
                            },
                        )
                    }
                }
            }
        }

        Row(
            Modifier.fillMaxWidth().padding(horizontal = Space.Gutter, vertical = Space.Hair),
            horizontalArrangement = Arrangement.spacedBy(18.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            LibraryViewTab("ALBUMS", albumsView) {
                viewOverride = "albums"
                env.library.setView("albums")
            }
            LibraryViewTab("ARTISTS", !albumsView) {
                viewOverride = "artists"
                if (selecting) exitSelect()
                env.library.setView("artists")
            }
            Spacer(Modifier.weight(1f))
            val snapCount = snapshot
            if (snapCount != null) {
                Text(
                    if (albumsView) pluralCount(snapCount.albums.size, "ALBUM", "ALBUMS")
                    else pluralCount(snapCount.artists.size, "ARTIST", "ARTISTS"),
                    color = Orbit.TextMute,
                    style = OrbitType.Counter,
                    maxLines = 1,
                )
            }
        }

        ScanStrip(scan) { env.library.cancelScan() }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            val snap = snapshot
            when {
                snap == null -> EmptyMessage("LOADING")
                snap.trackCount == 0u && scan !is ScanUi.Running -> {
                    Column(
                        Modifier.fillMaxSize(),
                        verticalArrangement = Arrangement.Center,
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        EmptyMessage("NO MUSIC YET")
                        Row(horizontalArrangement = Arrangement.spacedBy(Space.M)) {
                            OrbitButton("SCAN", env.scan, primary = true)
                            OrbitButton("ADD FOLDER", env.addFolder)
                        }
                    }
                }
                albumsView -> AlbumGrid(
                    snap, albumGridState, selecting, selected,
                    onOpen = { env.nav.push(Route.Album(it)) },
                    onToggle = { key -> selected = if (key in selected) selected - key else selected + key },
                    onStartSelect = { key ->
                        selecting = true
                        selected = setOf(key)
                    },
                )
                else -> ArtistList(snap, artistListState) { env.nav.push(Route.Artist(it)) }
            }
        }

        if (selecting) {
            val allKeys = remember(snapshot) { snapshot?.albums?.map { it.key }.orEmpty() }
            SelectionBar(
                count = selected.size,
                onSelectAll = { selected = allKeys.toSet() },
                onCancel = { exitSelect() },
                onDelete = { confirmDelete = true },
            )
        }
    }

    if (confirmDelete && selected.isNotEmpty()) {
        val n = selected.size
        ConfirmDialog(
            message = "REMOVE $n ${if (n == 1) "ALBUM" else "ALBUMS"} FROM THE LIBRARY? THE MUSIC FILES ARE NOT DELETED.",
            confirmLabel = "DELETE",
            onConfirm = {
                val keys = selected.toList()
                confirmDelete = false
                env.library.deleteAlbums(keys) { exitSelect() }
            },
            onDismiss = { confirmDelete = false },
        )
    }

    // Albums can vanish (rescan, delete): drop selected keys that no longer exist.
    LaunchedEffect(snapshot) {
        val present = snapshot?.albums?.mapTo(HashSet()) { it.key } ?: return@LaunchedEffect
        if (selected.any { it !in present }) selected = selected.filterTo(HashSet()) { it in present }
    }
}

@Composable
private fun AlbumGrid(
    snap: LibrarySnapshot,
    state: LazyGridState,
    selecting: Boolean,
    selected: Set<String>,
    onOpen: (String) -> Unit,
    onToggle: (String) -> Unit,
    onStartSelect: (String) -> Unit,
) {
    val enter = rememberEnterScope()
    LazyVerticalGrid(
        columns = GridCells.Adaptive(156.dp),
        state = state,
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = Space.L, end = Space.L, top = Space.S, bottom = Space.Xl),
        horizontalArrangement = Arrangement.spacedBy(Dimens.RowPadV),
        verticalArrangement = Arrangement.spacedBy(Space.M),
    ) {
        items(snap.albums, key = { it.key }, contentType = { "album" }) { album ->
            AlbumCard(
                album = album,
                selecting = selecting,
                selected = album.key in selected,
                onClick = { if (selecting) onToggle(album.key) else onOpen(album.key) },
                onLongClick = { if (selecting) onToggle(album.key) else onStartSelect(album.key) },
                modifier = Modifier.enterOnce(enter, album.key),
            )
        }
    }
}

@Composable
private fun ArtistList(snap: LibrarySnapshot, state: LazyListState, onOpen: (String) -> Unit) {
    val enter = rememberEnterScope()
    LazyColumn(modifier = Modifier.fillMaxSize(), state = state, contentPadding = PaddingValues(bottom = Space.Xl)) {
        items(snap.artists, key = { it.name }, contentType = { "artist" }) { artist ->
            ArtistRow(artist, onClick = { onOpen(artist.name) }, modifier = Modifier.enterOnce(enter, artist.name))
        }
    }
}

/** Thin strip under the header showing scan progress, failure or the last scan's result. */
@Composable
private fun ScanStrip(scan: ScanUi, onCancel: () -> Unit) {
    // The result hides itself after a while: LibraryViewModel clears it back to Idle.
    val env = LocalEnv.current
    val dismiss = { env.library.dismissScanResult() }

    when (scan) {
        ScanUi.Idle -> Unit
        is ScanUi.Running -> Column(Modifier.fillMaxWidth().padding(horizontal = Space.L, vertical = Space.Compact)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (scan.total > 0u) "${scan.phase.uppercase()} ${scan.scanned}/${scan.total}" else scan.phase.uppercase(),
                    modifier = Modifier.weight(1f),
                    color = Orbit.TextDim, style = OrbitType.Status,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                Box(
                    Modifier.heightIn(min = Dimens.Touch).clickable(onClick = onCancel).padding(horizontal = Space.S),
                    contentAlignment = Alignment.Center,
                ) {
                    Text("CANCEL", color = Orbit.Text, style = OrbitType.StatusBold)
                }
            }
            if (scan.total > 0u) {
                LinearProgressIndicator(
                    progress = { (scan.scanned.toFloat() / scan.total.toFloat()).coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth(),
                    color = LocalAccent.current,
                    trackColor = Orbit.Line,
                )
            }
        }
        is ScanUi.Failed -> Row(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = Space.L, vertical = Space.Compact)
                .background(Orbit.Danger.withAlpha(Alpha.Wash))
                .border(BorderStroke(Dimens.Stroke, Orbit.Danger.withAlpha(Alpha.BorderSoft)))
                .padding(start = Space.M, end = Space.Xs, top = Space.Hair, bottom = Space.Hair),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Text(
                scan.message,
                modifier = Modifier.weight(1f),
                color = Orbit.Danger,
                style = OrbitType.RowMeta,
            )
            Box(Modifier.size(Dimens.Touch).clickable(role = Role.Button) { dismiss() }, contentAlignment = Alignment.Center) {
                Icon(OrbitIcons.X, "Dismiss", tint = Orbit.Danger, modifier = Modifier.size(Dimens.IconS))
            }
        }
        is ScanUi.Done -> {
            val summary = scan.summary
            val parts = buildList {
                if (summary.addedTracks > 0u) add("+${summary.addedTracks} TRACKS")
                if (summary.newAlbums > 0u) add("+${summary.newAlbums} ALBUMS")
                if (summary.updatedTracks > 0u) add("${summary.updatedTracks} UPDATED")
                if (summary.removedTracks > 0u) add("-${summary.removedTracks} REMOVED")
                if (summary.covers > 0u) add("${summary.covers} COVERS")
            }
            val doneLabel = if (parts.isEmpty()) "LIBRARY UP TO DATE" else parts.joinToString(" · ")
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(horizontal = Space.L, vertical = Space.Compact)
                    .background(Orbit.Teal.withAlpha(Alpha.Wash))
                    .border(BorderStroke(Dimens.Stroke, Orbit.Teal.withAlpha(Alpha.BorderSoft)))
                    .padding(start = Space.M, end = Space.Xs, top = Space.Hair, bottom = Space.Hair),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Space.S),
            ) {
                Text(
                    doneLabel,
                    modifier = Modifier.weight(1f),
                    color = Orbit.Teal,
                    style = OrbitType.Counter,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Box(Modifier.size(Dimens.Touch).clickable(role = Role.Button) { dismiss() }, contentAlignment = Alignment.Center) {
                    Icon(OrbitIcons.X, "Dismiss", tint = Orbit.Teal, modifier = Modifier.size(Dimens.IconS))
                }
            }
        }
    }
}

/** Desktop `[PROJECT_LT] RL09` badge header (no teal subtitle) with a steady green status LED. */
@Composable
private fun TerminalHeader(
    actions: @Composable () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = Dimens.HeaderHeight)
            .padding(start = Space.Gutter, end = Space.Xs, top = Space.Compact, bottom = Space.Compact),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.S),
    ) {
        Box(
            Modifier
                .background(Orbit.Card)
                .border(BorderStroke(Dimens.Stroke, Orbit.Line))
                .padding(start = Dimens.RowPadV, end = Space.M, top = 7.dp, bottom = 7.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Dimens.RowPadV),
            ) {
                Box(
                    Modifier
                        .size(Space.Xxl)
                        .border(BorderStroke(Dimens.Stroke, Orbit.Orange)),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(OrbitIcons.Terminal, null, tint = Orbit.Orange, modifier = Modifier.size(13.dp))
                }
                // Title and LED share a tight row: the LED sits right after "RL09", raised to cap height.
                Row {
                    Text(
                        "[PROJECT_LT] RL09",
                        color = Orbit.Text,
                        style = OrbitType.TerminalTitle,
                        maxLines = 1,
                    )
                    StatusDot(Orbit.Green, Modifier.align(Alignment.Top).padding(start = Space.Hair), size = Space.Compact)
                }
            }
        }
        Spacer(Modifier.weight(1f))
        actions()
    }
}

/** Borderless view tab modeled after the desktop topbar: highlighted text + indicator dot when active. */
@Composable
private fun LibraryViewTab(
    label: String,
    active: Boolean,
    onClick: () -> Unit,
) {
    Row(
        Modifier
            .heightIn(min = Dimens.Touch)
            .pressable(pressedScale = 0.96f, role = Role.Tab, onClick = onClick)
            .padding(vertical = Space.Compact),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.Compact),
    ) {
        if (active) {
            Box(
                Modifier
                    .size(5.dp)
                    .clip(CircleShape)
                    .background(Orbit.Gold),
            )
        }
        Text(
            label,
            color = if (active) Orbit.Gold else Orbit.TextDim,
            style = if (active) OrbitType.ViewTabActive else OrbitType.ViewTab,
            maxLines = 1,
        )
    }
}

/** Bottom bar in select mode: count plus SELECT ALL / CANCEL / DELETE. */
@Composable
private fun SelectionBar(count: Int, onSelectAll: () -> Unit, onCancel: () -> Unit, onDelete: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Orbit.PanelDeep)) {
        Divider()
        Column(
            Modifier.fillMaxWidth().padding(horizontal = Space.L, vertical = Dimens.RowPadV),
            verticalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Text("$count SELECTED", color = Orbit.Gold, style = OrbitType.Button)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                OrbitButton("SELECT ALL", onSelectAll, Modifier.weight(1f))
                OrbitButton("CANCEL", onCancel, Modifier.weight(1f))
                OrbitButton("DELETE", onDelete, Modifier.weight(1f), danger = true, enabled = count > 0)
            }
        }
    }
}

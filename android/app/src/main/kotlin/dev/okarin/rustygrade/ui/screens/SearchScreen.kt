package dev.okarin.rustygrade.ui.screens

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.input.KeyboardActionHandler
import androidx.compose.foundation.text.input.TextFieldDecorator
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.ui.components.rememberSyncedTextState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.Kicker
import dev.okarin.rustygrade.ui.components.OrbitChip
import dev.okarin.rustygrade.ui.components.ScreenHeader
import dev.okarin.rustygrade.ui.components.TrackRow
import dev.okarin.rustygrade.ui.components.TrackActionSheet
import dev.okarin.rustygrade.ui.components.pluralCount
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.enterOnce
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.motion.rememberEnterScope
import dev.okarin.rustygrade.ui.nav.PlaylistKind
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import uniffi.rustygrade_ffi.SearchResults
import uniffi.rustygrade_ffi.TrackInfo

private val NoResults = SearchResults(emptyList(), emptyList(), emptyList(), emptyList(), emptyList())

private enum class SearchFilter(val label: String) {
    All("ALL"),
    Albums("ALBUMS"),
    Artists("ARTISTS"),
    Playlists("PLAYLISTS"),
    Tracks("TRACKS"),
}

/** Search tab: debounced search over albums, artists, playlists, smart playlists and tracks. */
@Composable
fun SearchScreen() {
    val env = LocalEnv.current
    val keyboard = LocalSoftwareKeyboardController.current
    val focusManager = LocalFocusManager.current
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf(SearchFilter.All) }
    var results by remember { mutableStateOf<SearchResults?>(null) }
    var sheetTrack by remember { mutableStateOf<TrackInfo?>(null) }

    // A rescan, delete, or playlist edit changes what exists: refresh the results of the current query too.
    val libraryVersion by env.library.library.collectAsStateWithLifecycle()
    val collectionsVersion by env.collections.collections.collectAsStateWithLifecycle()
    LaunchedEffect(query, libraryVersion, collectionsVersion) {
        if (query.isBlank()) {
            results = null
            return@LaunchedEffect
        }
        delay(250)
        results = try {
            io { env.engine.searchLibrary(query) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            NoResults
        }
    }

    val hideKeyboardOnScroll = remember(keyboard) {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (available.y != 0f) keyboard?.hide()
                return Offset.Zero
            }
        }
    }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("SEARCH")
        SearchField(
            query = query,
            onQueryChange = { query = it },
            onSearch = {
                keyboard?.hide()
                focusManager.clearFocus()
            },
        )

        val r = results
        val playlistHitsCount = if (r == null) 0 else r.playlists.size + r.smartPlaylists.size
        val hasAnyResults = r != null &&
            (r.albums.isNotEmpty() || r.artists.isNotEmpty() || playlistHitsCount > 0 || r.tracks.isNotEmpty())
        if (hasAnyResults) {
            val total = r.albums.size + r.artists.size + playlistHitsCount + r.tracks.size
            Row(
                Modifier
                    .fillMaxWidth()
                    .horizontalScroll(rememberScrollState())
                    .padding(horizontal = Space.Gutter, vertical = Space.Xs),
                horizontalArrangement = Arrangement.spacedBy(Space.S),
            ) {
                val counts = listOf(
                    SearchFilter.All to total,
                    SearchFilter.Albums to r.albums.size,
                    SearchFilter.Artists to r.artists.size,
                    SearchFilter.Playlists to playlistHitsCount,
                    SearchFilter.Tracks to r.tracks.size,
                )
                counts.forEach { (tab, count) ->
                    if (tab == SearchFilter.All || count > 0) {
                        OrbitChip("${tab.label} ($count)", active = filter == tab, onClick = { filter = tab })
                    }
                }
            }
        }

        when {
            query.isBlank() -> IdlePanel("AWAITING_QUERY", "TYPE TO SEARCH ALBUMS, ARTISTS, PLAYLISTS, TRACKS")
            r == null -> Unit
            !hasAnyResults -> IdlePanel("NO_MATCH", "NO RESULTS")
            else -> {
                val showAlbums = (filter == SearchFilter.All || filter == SearchFilter.Albums) && r.albums.isNotEmpty()
                val showArtists = (filter == SearchFilter.All || filter == SearchFilter.Artists) && r.artists.isNotEmpty()
                val showPlaylists = (filter == SearchFilter.All || filter == SearchFilter.Playlists) && playlistHitsCount > 0
                val showTracks = (filter == SearchFilter.All || filter == SearchFilter.Tracks) && r.tracks.isNotEmpty()
                if (!showAlbums && !showArtists && !showPlaylists && !showTracks) {
                    IdlePanel("NO_MATCH", "NO RESULTS IN ${filter.label}")
                } else {
                    val trackIds = remember(r) { r.tracks.map { it.id } }
                    // A fresh cascade per query; a refresh of the same query (rescan, playlist edit) keeps
                    // the scope, so rows whose key survives it do not replay.
                    val enter = rememberEnterScope(query)
                    val headerPad = Modifier.padding(start = Space.Gutter, end = Space.Gutter, top = Space.L, bottom = Space.S)
                    LazyColumn(
                        Modifier.weight(1f).fillMaxWidth().nestedScroll(hideKeyboardOnScroll),
                        contentPadding = PaddingValues(bottom = Space.L),
                    ) {
                        if (showAlbums) {
                            item(key = "h-albums", contentType = "section") { Kicker("ALBUMS", headerPad) }
                            items(r.albums, key = { "a:${it.key}" }, contentType = { "result" }) { album ->
                                ResultRow(
                                    thumb = album.thumbPath,
                                    title = album.name,
                                    subtitle = album.artist,
                                    onClick = { env.nav.push(Route.Album(album.key)) },
                                    modifier = Modifier.enterOnce(enter, "a:${album.key}"),
                                )
                            }
                        }
                        if (showArtists) {
                            item(key = "h-artists", contentType = "section") { Kicker("ARTISTS", headerPad) }
                            items(r.artists, key = { "r:${it.name}" }, contentType = { "result" }) { artist ->
                                ResultRow(
                                    thumb = artist.thumbPath,
                                    title = artist.name,
                                    subtitle = "${pluralCount(artist.albumCount, "ALBUM", "ALBUMS")} · ${pluralCount(artist.trackCount, "TRACK", "TRACKS")}",
                                    onClick = { env.nav.push(Route.Artist(artist.name)) },
                                    modifier = Modifier.enterOnce(enter, "r:${artist.name}"),
                                )
                            }
                        }
                        if (showPlaylists) {
                            item(key = "h-playlists", contentType = "section") { Kicker("PLAYLISTS", headerPad) }
                            items(r.playlists, key = { "pl:${it.id}" }, contentType = { "result" }) { playlist ->
                                ResultRow(
                                    thumb = playlist.coverPath,
                                    title = playlist.name,
                                    subtitle = pluralCount(playlist.trackIds.size, "TRACK", "TRACKS"),
                                    onClick = { env.nav.push(Route.Playlist(PlaylistKind.Custom(playlist.id))) },
                                    modifier = Modifier.enterOnce(enter, "pl:${playlist.id}"),
                                )
                            }
                            items(r.smartPlaylists, key = { "sm:${it.id}" }, contentType = { "result" }) { smart ->
                                val mode = if (smart.matchAll) "MATCH ALL" else "MATCH ANY"
                                ResultRow(
                                    lead = { IconBlock(OrbitIcons.Filter, Orbit.Gold, Dimens.SheetCover) },
                                    title = smart.name,
                                    subtitle = "SMART · $mode · ${pluralCount(smart.rules.size, "RULE", "RULES")}",
                                    onClick = { env.nav.push(Route.Playlist(PlaylistKind.Smart(smart.id))) },
                                    modifier = Modifier.enterOnce(enter, "sm:${smart.id}"),
                                )
                            }
                        }
                        if (showTracks) {
                            item(key = "h-tracks", contentType = "section") { Kicker("TRACKS", headerPad) }
                            itemsIndexed(r.tracks, key = { index, t -> "t:$index:${t.id}" }, contentType = { _, _ -> "track" }) { index, track ->
                                TrackRow(
                                    track = track,
                                    showCover = true,
                                    onClick = { env.playback.playTracks(trackIds, index) },
                                    onMore = { sheetTrack = track },
                                    modifier = Modifier.enterOnce(enter, "t:$index:${track.id}"),
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    sheetTrack?.let { TrackActionSheet(it, onDismiss = { sheetTrack = null }) }
}

private val GoldCursor = SolidColor(Orbit.Gold)

@Composable
private fun SearchField(query: String, onQueryChange: (String) -> Unit, onSearch: () -> Unit) {
    var focused by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = Space.Gutter, vertical = Space.Compact)
            .background(Orbit.Panel)
            .border(BorderStroke(Dimens.Stroke, if (focused) Orbit.Gold else Orbit.Line))
            .heightIn(min = 48.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(OrbitIcons.Search, null, tint = if (focused) Orbit.Gold else Orbit.TextDim, modifier = Modifier.padding(start = Space.M).size(Dimens.IconMd))
        val state = rememberSyncedTextState(query, onQueryChange)
        BasicTextField(
            state = state,
            modifier = Modifier.weight(1f).padding(horizontal = Dimens.RowPadV).onFocusChanged { focused = it.isFocused },
            lineLimits = TextFieldLineLimits.SingleLine,
            textStyle = OrbitType.FieldSearch,
            cursorBrush = GoldCursor,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            onKeyboardAction = KeyboardActionHandler { onSearch() },
            decorator = TextFieldDecorator { inner ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (state.text.isEmpty()) {
                        Text("SEARCH LIBRARY", color = Orbit.TextMute, style = OrbitType.Hud)
                    }
                    inner()
                }
            },
        )
        if (query.isNotEmpty()) {
            Box(Modifier.size(48.dp).pressable(pressedScale = 0.9f) { onQueryChange("") }, contentAlignment = Alignment.Center) {
                Icon(OrbitIcons.X, "Clear", tint = Orbit.TextDim, modifier = Modifier.size(Dimens.IconS))
            }
        }
    }
}

/**
 * The Lonetrail terminal idle block shown instead of results: a large dim search glyph, a teal
 * `— KICKER` and one HUD line, framed like a HUD panel.
 */
@Composable
private fun IdlePanel(kicker: String, line: String) {
    Box(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = Space.Gutter, vertical = Space.L),
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .background(Orbit.Panel)
                .border(BorderStroke(Dimens.Stroke, Orbit.LineSoft))
                .padding(horizontal = Space.L, vertical = Space.Xxl),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Space.M),
        ) {
            Icon(OrbitIcons.Search, null, tint = Orbit.TextMute.withAlpha(Alpha.Border), modifier = Modifier.size(40.dp))
            Kicker(kicker)
            Text(line, color = Orbit.TextDim, style = OrbitType.Hud, textAlign = TextAlign.Center)
        }
        CornerBrackets(Modifier.matchParentSize(), length = Dimens.TickS)
    }
}

@Composable
private fun ResultRow(
    title: String,
    subtitle: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    thumb: String? = null,
    lead: (@Composable () -> Unit)? = null,
) {
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = Dimens.Touch)
                .pressable(pressedScale = 0.985f, onClick = onClick)
                .padding(horizontal = Space.Gutter, vertical = Space.S),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.M),
        ) {
            if (lead != null) lead() else Cover(thumb, size = Dimens.SheetCover)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                Text(
                    title,
                    color = Orbit.Text,
                    style = OrbitType.RowTitle,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(subtitle, color = Orbit.TextDim, style = OrbitType.RowMeta, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Icon(OrbitIcons.Chevrondown, null, tint = Orbit.TextMute, modifier = Modifier.size(Dimens.IconS).rotate(-90f))
        }
        Divider(Modifier.padding(horizontal = Space.Gutter))
    }
}

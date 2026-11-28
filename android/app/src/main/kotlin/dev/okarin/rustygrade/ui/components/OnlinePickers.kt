package dev.okarin.rustygrade.ui.components

import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.storage.ArtworkCandidate
import dev.okarin.rustygrade.storage.ArtworkProviders
import dev.okarin.rustygrade.storage.ArtworkQuery
import dev.okarin.rustygrade.storage.LyricCandidate
import dev.okarin.rustygrade.storage.LyricQuery
import dev.okarin.rustygrade.storage.OnlineArtworkLookup
import dev.okarin.rustygrade.storage.OnlineLyricsLookup
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.withAlpha
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import uniffi.rustygrade_ffi.TrackInfo

/**
 * Interactive online lyrics picker searching LRCLIB, NetEase, KuGou, and Lyrics.ovh.
 * Tapping a result saves it to the persistent lyrics cache and calls [onSaved].
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LyricPickerSheet(
    track: TrackInfo,
    onDismiss: () -> Unit,
    onSaved: () -> Unit = {},
    onPickedText: ((String) -> Unit)? = null,
) {
    val env = LocalEnv.current
    val scope = rememberCoroutineScope()

    var titleQuery by remember(track.id) { mutableStateOf(track.title) }
    var artistQuery by remember(track.id) { mutableStateOf(track.artist) }
    var searchTick by remember(track.id) { mutableIntStateOf(0) }

    var loading by remember { mutableStateOf(true) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var results by remember { mutableStateOf<List<LyricCandidate>>(emptyList()) }

    LaunchedEffect(track.id, searchTick) {
        loading = true
        error = null
        results = try {
            OnlineLyricsLookup.searchAll(
                LyricQuery(
                    title = titleQuery,
                    artist = artistQuery,
                    album = track.album,
                    durationSecs = track.durationSecs,
                ),
            )
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = (e.message ?: "SEARCH FAILED").uppercase()
            emptyList()
        } finally {
            loading = false
        }
    }

    ModalBottomSheet(
        onDismissRequest = { if (!saving) onDismiss() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = RectangleShape,
        containerColor = Orbit.PanelDeep,
        contentColor = Orbit.Text,
        dragHandle = { BottomSheetDefaults.DragHandle(color = Orbit.Line) },
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(0.84f)
                .navigationBarsPadding()
                .padding(horizontal = Space.Gutter)
                .padding(bottom = Space.L),
            verticalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Kicker("ONLINE LYRICS")
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Xs)) {
                OrbitTextField(
                    value = titleQuery,
                    onValueChange = { titleQuery = it },
                    modifier = Modifier.weight(1f),
                    placeholder = "TITLE",
                )
                OrbitTextField(
                    value = artistQuery,
                    onValueChange = { artistQuery = it },
                    modifier = Modifier.weight(1f),
                    placeholder = "ARTIST",
                )
            }
            OrbitButton(
                if (loading) "SEARCHING…" else "SEARCH PROVIDERS",
                onClick = { if (!loading && !saving) searchTick++ },
                modifier = Modifier.fillMaxWidth(),
                icon = OrbitIcons.Search,
                enabled = !loading && !saving && titleQuery.isNotBlank(),
            )
            error?.let {
                Text(it, color = Orbit.Danger, style = OrbitType.Hud)
            }
            Divider()

            when {
                loading -> Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                    Text("QUERYING LRCLIB // NETEASE // KUGOU // OVH…", color = Orbit.Gold, style = OrbitType.Hud)
                }
                results.isEmpty() -> Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                    EmptyMessage("NO LYRICS FOUND — TRY EDITING THE QUERY")
                }
                else -> LazyColumn(
                    Modifier.fillMaxWidth().weight(1f),
                    verticalArrangement = Arrangement.spacedBy(Space.S),
                ) {
                    itemsIndexed(results, key = { i, c -> "$i:${c.source}" }, contentType = { _, _ -> "lyric" }) { _, cand ->
                        LyricCandidateCard(
                            candidate = cand,
                            enabled = !saving,
                            onClick = {
                                if (saving) return@LyricCandidateCard
                                if (onPickedText != null) {
                                    onPickedText(cand.text)
                                    onDismiss()
                                    return@LyricCandidateCard
                                }
                                saving = true
                                error = null
                                scope.launch {
                                    try {
                                        io { env.engine.saveTrackLyrics(track.id, cand.text) }
                                        onSaved()
                                        onDismiss()
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        error = (e.message ?: "COULD NOT SAVE LYRICS").uppercase()
                                        saving = false
                                    }
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun LyricCandidateCard(
    candidate: LyricCandidate,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .background(Orbit.Panel)
            .border(BorderStroke(Dimens.Stroke, if (candidate.synced) Orbit.Teal.withAlpha(Alpha.BorderSoft) else Orbit.LineSoft))
            .then(if (enabled) Modifier.pressable(pressedScale = 0.985f, onClick = onClick) else Modifier)
            .padding(Space.M),
        verticalArrangement = Arrangement.spacedBy(Space.Xs),
    ) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(Space.Compact),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SourceBadge(candidate.source, Orbit.Gold)
            SourceBadge(
                if (candidate.synced) "SYNCED LRC" else "PLAIN",
                if (candidate.synced) Orbit.Teal else Orbit.TextDim,
            )
            Spacer(Modifier.weight(1f))
            candidate.durationSecs?.let { dur ->
                Text(formatDuration(dur), color = Orbit.TextDim, style = OrbitType.Numeric)
            }
        }
        Text(
            candidate.title.ifBlank { "Untitled" },
            color = Orbit.Text,
            style = OrbitType.RowTitle,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        val sub = listOfNotNull(
            candidate.artist.ifBlank { null },
            candidate.album?.ifBlank { null },
        ).joinToString(" · ")
        if (sub.isNotEmpty()) {
            Text(
                sub,
                color = Orbit.TextDim,
                style = OrbitType.RowMeta,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (candidate.preview.isNotEmpty()) {
            Text(
                candidate.preview,
                color = Orbit.TextMute,
                style = OrbitType.RowMeta,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/**
 * Interactive online cover picker searching iTunes, Deezer, and MusicBrainz/Cover Art Archive.
 * Downloads the high-resolution cover to a temp file and hands it to [onPickedFile].
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CoverPickerSheet(
    initialAlbum: String,
    initialArtist: String,
    trackTitle: String = "",
    year: Int = 0,
    onDismiss: () -> Unit,
    onPickedFile: (File) -> Unit,
) {
    val env = LocalEnv.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val settings by env.library.settings.collectAsStateWithLifecycle()

    var albumQuery by remember(initialAlbum) { mutableStateOf(initialAlbum) }
    var artistQuery by remember(initialArtist) { mutableStateOf(initialArtist) }
    var searchTick by remember(initialAlbum, initialArtist) { mutableIntStateOf(0) }

    var loading by remember { mutableStateOf(true) }
    var downloading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var results by remember { mutableStateOf<List<ArtworkCandidate>>(emptyList()) }

    LaunchedEffect(initialAlbum, initialArtist, searchTick) {
        loading = true
        error = null
        val s = settings
        val providers = ArtworkProviders(
            itunes = s?.artworkItunes ?: true,
            deezer = s?.artworkDeezer ?: true,
            musicbrainz = s?.artworkMusicbrainz ?: true,
        )
        results = try {
            OnlineArtworkLookup.searchAll(
                ArtworkQuery(
                    album = albumQuery,
                    artist = artistQuery,
                    trackTitle = trackTitle,
                    year = year,
                    providers = providers,
                ),
            )
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = (e.message ?: "SEARCH FAILED").uppercase()
            emptyList()
        } finally {
            loading = false
        }
    }

    ModalBottomSheet(
        onDismissRequest = { if (!downloading) onDismiss() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = RectangleShape,
        containerColor = Orbit.PanelDeep,
        contentColor = Orbit.Text,
        dragHandle = { BottomSheetDefaults.DragHandle(color = Orbit.Line) },
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(0.86f)
                .navigationBarsPadding()
                .padding(horizontal = Space.Gutter)
                .padding(bottom = Space.L),
            verticalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Kicker("ONLINE COVERS")
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.Xs)) {
                OrbitTextField(
                    value = albumQuery,
                    onValueChange = { albumQuery = it },
                    modifier = Modifier.weight(1f),
                    placeholder = "ALBUM",
                )
                OrbitTextField(
                    value = artistQuery,
                    onValueChange = { artistQuery = it },
                    modifier = Modifier.weight(1f),
                    placeholder = "ARTIST",
                )
            }
            OrbitButton(
                if (loading) "SEARCHING…" else "SEARCH COVERS",
                onClick = { if (!loading && !downloading) searchTick++ },
                modifier = Modifier.fillMaxWidth(),
                icon = OrbitIcons.Search,
                enabled = !loading && !downloading && (albumQuery.isNotBlank() || trackTitle.isNotBlank()),
            )
            if (downloading) {
                Text("DOWNLOADING HIGH-RES COVER…", color = Orbit.Gold, style = OrbitType.Hud)
            }
            error?.let {
                Text(it, color = Orbit.Danger, style = OrbitType.Hud)
            }
            Divider()

            when {
                loading -> Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                    Text("QUERYING ITUNES // DEEZER // COVER ART ARCHIVE…", color = Orbit.Gold, style = OrbitType.Hud)
                }
                results.isEmpty() -> Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                    EmptyMessage("NO COVERS FOUND — TRY EDITING THE QUERY")
                }
                else -> {
                    val rows = remember(results) { results.chunked(2) }
                    LazyColumn(
                        Modifier.fillMaxWidth().weight(1f),
                        verticalArrangement = Arrangement.spacedBy(Space.S),
                    ) {
                        itemsIndexed(rows, key = { i, pair -> "$i:" + pair.joinToString("|") { it.thumbUrl } }, contentType = { _, _ -> "cover-row" }) { _, pair ->
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                                pair.forEach { cand ->
                                    val isBest = rows.firstOrNull()?.firstOrNull() == cand && cand.score >= 65
                                    CoverCandidateTile(
                                        candidate = cand,
                                        isBest = isBest,
                                        enabled = !downloading,
                                        modifier = Modifier.weight(1f),
                                        onClick = {
                                            if (downloading) return@CoverCandidateTile
                                            downloading = true
                                            error = null
                                            scope.launch {
                                                val file = try {
                                                    OnlineArtworkLookup.downloadCoverToFile(cand, context.cacheDir)
                                                } catch (e: CancellationException) {
                                                    throw e
                                                } catch (_: Exception) {
                                                    null
                                                }
                                                if (file != null) {
                                                    onPickedFile(file)
                                                    onDismiss()
                                                } else {
                                                    downloading = false
                                                    error = "COVER DOWNLOAD FAILED — TRY ANOTHER RESULT"
                                                }
                                            }
                                        },
                                    )
                                }
                                if (pair.size == 1) Spacer(Modifier.weight(1f))
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun CoverCandidateTile(
    candidate: ArtworkCandidate,
    isBest: Boolean,
    enabled: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val url = candidate.thumbUrl
    val thumb by produceState<ImageBitmap?>(ThumbCache.get(url), url) {
        // Re-read per URL: produceState keeps its value when only the key changes.
        value = ThumbCache.get(url)
        if (value != null) return@produceState
        val bytes = OnlineArtworkLookup.downloadThumbnailBytes(url) ?: return@produceState
        val bmp = io { decodeThumb(bytes) } ?: return@produceState
        ThumbCache.put(url, bmp)
        value = bmp
    }

    Column(
        modifier
            .background(Orbit.Panel)
            .border(BorderStroke(Dimens.Stroke, if (isBest) Orbit.Teal else Orbit.LineSoft))
            .then(if (enabled) Modifier.pressable(pressedScale = 0.97f, onClick = onClick) else Modifier)
            .padding(Space.S),
        verticalArrangement = Arrangement.spacedBy(Space.Compact),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .background(Orbit.Card)
                .border(BorderStroke(Dimens.Stroke, Orbit.LineSoft)),
            contentAlignment = Alignment.Center,
        ) {
            val img = thumb
            if (img != null) {
                Image(
                    bitmap = img,
                    contentDescription = candidate.title,
                    modifier = Modifier.fillMaxSize(),
                    contentScale = ContentScale.Crop,
                )
            } else {
                Text("LOADING", color = Orbit.TextMute, style = OrbitType.Hud)
            }
            Row(
                Modifier
                    .align(Alignment.TopStart)
                    .padding(Space.Xs),
                horizontalArrangement = Arrangement.spacedBy(Space.Xs),
            ) {
                if (isBest) SourceBadge("BEST", Orbit.Teal)
                SourceBadge(candidate.source, Orbit.Gold)
            }
        }
        Text(
            candidate.title.ifBlank { "Untitled" },
            color = Orbit.Text,
            style = OrbitType.TileTitle,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        val sub = listOfNotNull(candidate.artist.ifBlank { null }, candidate.year).joinToString(" · ")
        Text(
            sub.ifBlank { " " },
            color = Orbit.TextDim,
            style = OrbitType.PreviewMeta,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun SourceBadge(label: String, color: Color) {
    Text(
        label,
        modifier = Modifier
            .background(Orbit.Void.withAlpha(Alpha.TextOnArt))
            .border(BorderStroke(Dimens.Stroke, color.withAlpha(Alpha.Border)))
            .padding(horizontal = Space.Compact, vertical = Space.Hair),
        color = color,
        style = OrbitType.MicroBadge,
        maxLines = 1,
    )
}

/** Decoded candidate thumbnails by URL: scrolling back up neither refetches nor re-decodes them. */
private val ThumbCache = LruCache<String, ImageBitmap>(24)

/** Thumbnails are decoded at about this size: a grid tile never needs the full-size image. */
private const val ThumbTargetPx = 320

/** Decodes [bytes] subsampled by a power of two down to roughly [ThumbTargetPx], to bound memory. */
private fun decodeThumb(bytes: ByteArray): ImageBitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= ThumbTargetPx && bounds.outHeight / (sample * 2) >= ThumbTargetPx) sample *= 2
    val options = BitmapFactory.Options().apply { inSampleSize = sample }
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.asImageBitmap()
}

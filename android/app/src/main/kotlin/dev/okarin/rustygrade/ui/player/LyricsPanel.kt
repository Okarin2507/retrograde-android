package dev.okarin.rustygrade.ui.player

import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.currentStateAsState
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.storage.ContentResolverOpener
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.LocalIsPlaying
import dev.okarin.rustygrade.ui.components.LyricPickerSheet
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import uniffi.rustygrade_ffi.LyricLineInfo
import uniffi.rustygrade_ffi.TrackInfo

/** How long after the user lets go of the list the view starts following playback again. */
private const val RESUME_FOLLOW_MS = 3_000L

private sealed interface LyricsLoad {
    data object Loading : LyricsLoad
    data object None : LyricsLoad
    class Ready(val lines: List<LyricLineInfo>, val synced: Boolean, val source: String) : LyricsLoad
}

/**
 * Lyrics for [track]: synced lines follow playback (tap a line to seek), plain lyrics just scroll.
 * Loading happens off the main thread and is redone when the track changes or online lyrics are fetched.
 */
@Composable
fun LyricsPanel(track: TrackInfo, modifier: Modifier = Modifier, active: Boolean = true) {
    val env = LocalEnv.current
    val resolver = LocalContext.current.contentResolver
    val scope = rememberCoroutineScope()
    var reloadTick by remember(track.id) { mutableIntStateOf(0) }
    var showPicker by remember(track.id) { mutableStateOf(false) }

    val load by produceState<LyricsLoad>(LyricsLoad.Loading, track.id, reloadTick) {
        value = LyricsLoad.Loading
        val info = try {
            io { env.engine.lyrics(track.id, ContentResolverOpener(resolver)) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            null
        }
        value = when {
            info == null || info.lines.isEmpty() -> LyricsLoad.None
            info.synced -> {
                val timed = info.lines.filter { it.timeS >= 0.0 }
                if (timed.isEmpty()) LyricsLoad.Ready(info.lines, false, info.source)
                else LyricsLoad.Ready(timed, true, info.source)
            }
            else -> LyricsLoad.Ready(info.lines, false, info.source)
        }
    }

    Box(modifier.fillMaxSize()) {
        when (val l = load) {
            LyricsLoad.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("LOADING…", color = Orbit.TextDim, style = OrbitType.Section)
            }
            LyricsLoad.None -> Column(
                Modifier.fillMaxSize().padding(horizontal = Space.Xxl),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(Space.M, Alignment.CenterVertically),
            ) {
                EmptyMessage("NO LYRICS FOUND")
                OrbitButton(
                    "FETCH LYRICS ONLINE",
                    onClick = { showPicker = true },
                    primary = true,
                    icon = OrbitIcons.Download,
                )
                Text(
                    "Searches LRCLIB, NetEase, KuGou and Lyrics.ovh and caches the result locally.",
                    color = Orbit.TextDim,
                    style = OrbitType.RowMeta,
                    textAlign = TextAlign.Center,
                )
            }
            is LyricsLoad.Ready -> Column(Modifier.fillMaxSize()) {
                Row(
                    Modifier.fillMaxWidth().padding(bottom = Space.Xs),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Space.Xs),
                ) {
                    Text(
                        "SRC: ${l.source.uppercase()}",
                        color = Orbit.TextDim,
                        style = OrbitType.Hud,
                    )
                    Spacer(Modifier.weight(1f))
                    if (l.source.equals("CACHE", ignoreCase = true)) {
                        Text(
                            "RESET",
                            modifier = Modifier
                                .pressable(pressedScale = 0.94f) {
                                    scope.launch {
                                        io { env.engine.clearCachedLyrics(track.id) }
                                        reloadTick++
                                    }
                                }
                                .touchLabel(),
                            color = Orbit.TextDim,
                            style = OrbitType.Hud,
                        )
                    }
                    Text(
                        "FETCH",
                        modifier = Modifier
                            .pressable(pressedScale = 0.94f) { showPicker = true }
                            .touchLabel(),
                        color = Orbit.Gold,
                        style = OrbitType.Hud,
                    )
                }
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    androidx.compose.runtime.key(track.id, reloadTick) {
                        if (l.synced) SyncedLyrics(l.lines, active = active) else PlainLyrics(l.lines)
                    }
                }
            }
        }
    }

    if (showPicker) {
        LyricPickerSheet(
            track = track,
            onDismiss = { showPicker = false },
            onSaved = { reloadTick++ },
        )
    }
}

/** A small text action grown to a full touch target, its label centred vertically. */
private fun Modifier.touchLabel(): Modifier =
    this
        .heightIn(min = Dimens.Touch)
        .wrapContentHeight(Alignment.CenterVertically)
        .padding(horizontal = Space.S)

/** Fades the list out at its top and bottom edges. The mask is rebuilt only when the size changes. */
private fun Modifier.softVerticalFade(): Modifier =
    this
        .graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)
        .drawWithCache {
            val mask = if (size.height <= 0f) {
                null
            } else {
                val topEnd = (Space.L.toPx() / size.height).coerceIn(0f, 0.25f)
                val botStart = (1f - 28.dp.toPx() / size.height).coerceIn(0.7f, 1f)
                Brush.verticalGradient(
                    0f to Color.Transparent,
                    topEnd to Color.Black,
                    botStart to Color.Black,
                    1f to Color.Transparent,
                )
            }
            onDrawWithContent {
                drawContent()
                if (mask != null) drawRect(brush = mask, blendMode = BlendMode.DstIn)
            }
        }

@Composable
private fun PlainLyrics(lines: List<LyricLineInfo>) {
    LazyColumn(
        Modifier.fillMaxSize().softVerticalFade(),
        contentPadding = PaddingValues(vertical = Space.Xl),
    ) {
        itemsIndexed(lines, key = { i, _ -> i }, contentType = { _, _ -> "lyric" }) { _, line ->
            if (line.text.isBlank()) {
                Spacer(Modifier.height(Space.L))
            } else {
                Text(
                    line.text,
                    modifier = Modifier.fillMaxWidth().padding(vertical = 5.dp),
                    color = Orbit.Text.withAlpha(Alpha.LyricPlain),
                    style = OrbitType.LyricLine,
                )
            }
        }
    }
}

/** Index of the last line whose time is <= [positionMs], or -1 before the first line. Lines are sorted. */
private fun currentLineIndex(lines: List<LyricLineInfo>, positionMs: Long): Int {
    var lo = 0
    var hi = lines.size - 1
    var found = -1
    while (lo <= hi) {
        val mid = (lo + hi) ushr 1
        if (lines[mid].timeS * 1000.0 <= positionMs) {
            found = mid
            lo = mid + 1
        } else {
            hi = mid - 1
        }
    }
    return found
}

@Composable
private fun SyncedLyrics(lines: List<LyricLineInfo>, active: Boolean) {
    val env = LocalEnv.current
    val isPlaying = LocalIsPlaying.current
    val resumed = LocalLifecycleOwner.current.lifecycle.currentStateAsState().value.isAtLeast(Lifecycle.State.RESUMED)
    val playerOpen = env.nav.playerOpen
    // Lyrics only need to move while they can be seen: active tab, foregrounded, player open.
    val visible = active && resumed && playerOpen
    val listState = rememberLazyListState()

    // Playback position for the highlight. Sampled at 150 ms only while playing and visible;
    // paused, covered or backgrounded, it only moves on a seek.
    var positionMs by remember { mutableLongStateOf(PlaybackHub.estimatedPositionMs()) }
    LaunchedEffect(isPlaying, visible) {
        if (!visible) return@LaunchedEffect
        positionMs = PlaybackHub.estimatedPositionMs() // catch up once on coming back into view
        while (isPlaying && isActive) {
            delay(150)
            positionMs = PlaybackHub.estimatedPositionMs()
        }
    }
    LaunchedEffect(visible) {
        if (!visible) return@LaunchedEffect
        PlaybackHub.positionMs.collect { positionMs = PlaybackHub.estimatedPositionMs() }
    }
    val current by remember(lines) { derivedStateOf { currentLineIndex(lines, positionMs) } }

    // Auto-follow: pauses while the user drags or flings, resumes a few seconds after they let go.
    var following by remember { mutableStateOf(true) }
    var autoScrolling by remember { mutableStateOf(false) }
    var firstScroll by remember { mutableStateOf(true) }
    LaunchedEffect(listState) {
        // A touch on the list always wins, even while our own scroll animation is still running.
        listState.interactionSource.interactions.collect { if (it is DragInteraction.Start) following = false }
    }
    LaunchedEffect(listState) {
        snapshotFlow { listState.isScrollInProgress }.collectLatest { scrolling ->
            if (scrolling) {
                if (!autoScrolling) following = false
            } else if (!following) {
                delay(RESUME_FOLLOW_MS)
                following = true
            }
        }
    }
    var wasVisible by remember { mutableStateOf(true) }
    LaunchedEffect(current, following, visible) {
        if (!visible) {
            wasVisible = false
            return@LaunchedEffect
        }
        if (!following || current < 0) return@LaunchedEffect
        snapshotFlow { listState.layoutInfo.viewportSize.height }.first { it > 0 }
        val offset = -(listState.layoutInfo.viewportSize.height / 3)
        autoScrolling = true
        try {
            // The first jump, and the one after coming back into view, must not animate.
            if (firstScroll || !wasVisible) {
                firstScroll = false
                wasVisible = true
                listState.scrollToItem(current, offset)
            } else {
                listState.animateScrollToItem(current, offset)
            }
        } finally {
            autoScrolling = false
        }
    }

    val activeColor = rememberReadableAccentOn(Orbit.Void)
    LazyColumn(
        state = listState,
        modifier = Modifier.fillMaxSize().softVerticalFade(),
        contentPadding = PaddingValues(top = Space.Xl, bottom = 180.dp),
    ) {
        itemsIndexed(lines, key = { i, _ -> i }, contentType = { _, _ -> "lyric" }) { i, line ->
            val cur = current
            val color = when {
                i == cur -> activeColor
                // Sung lines stay at Muted: still WCAG AA on the void.
                i < cur -> Orbit.Text.withAlpha(Alpha.Muted)
                else -> Orbit.Text.withAlpha(Alpha.LyricUpcoming)
            }
            Text(
                line.text.ifBlank { "·" },
                modifier = Modifier
                    .fillMaxWidth()
                    .pressable(pressedScale = 0.99f) {
                        positionMs = (line.timeS * 1000.0).toLong()
                        following = true
                        env.playback.seekTo((line.timeS * 1000).toLong())
                    }
                    .padding(vertical = Dimens.RowPadV),
                color = color,
                style = when {
                    i == cur -> OrbitType.LyricActive
                    i > cur -> OrbitType.LyricMid
                    else -> OrbitType.LyricLine
                },
            )
        }
    }
}

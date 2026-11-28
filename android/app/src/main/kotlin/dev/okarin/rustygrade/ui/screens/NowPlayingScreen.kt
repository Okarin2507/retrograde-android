package dev.okarin.rustygrade.ui.screens

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.media.UsbStatus
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.LocalPowerSaveMode
import dev.okarin.rustygrade.ui.components.BitPerfectGlyph
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.HexButton
import dev.okarin.rustygrade.ui.components.IconTap
import dev.okarin.rustygrade.ui.components.LocalLikedIds
import dev.okarin.rustygrade.ui.components.QualityPill
import dev.okarin.rustygrade.ui.components.SeekBar
import dev.okarin.rustygrade.ui.components.TrackActionSheet
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.player.LyricsPanel
import dev.okarin.rustygrade.ui.player.PlayerBackdrop
import dev.okarin.rustygrade.ui.player.QueuePanel
import dev.okarin.rustygrade.ui.player.VolumeEqSheet
import dev.okarin.rustygrade.ui.player.liquidContentAlpha
import dev.okarin.rustygrade.ui.player.liquidReveal
import dev.okarin.rustygrade.ui.player.rememberLiquidReveal
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.orbitTween
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import uniffi.rustygrade_ffi.TrackInfo

/** Portrait heights below this show the compact player controls. */
private val ShortScreen = 640.dp

private enum class PlayerTab(val label: String) {
    Art("ART"),
    Lyrics("LYRICS"),
    Queue("QUEUE"),
}

/**
 * Full-screen player. It is revealed by a liquid blot growing from the mini-bar's play button
 * (and folding back into it on close), over a blurred-cover backdrop. A tab strip switches the
 * main area between the cover, the lyrics and the queue. Portrait stacks everything; landscape
 * puts the tabbed area on the left and the controls on the right.
 */
@Composable
fun NowPlayingScreen(onClose: () -> Unit) {
    val env = LocalEnv.current
    val state by PlaybackHub.state.collectAsStateWithLifecycle()
    val track = state?.current
    LaunchedEffect(track == null) { if (track == null) onClose() }

    // The shell flips nav.playerOpen for the chevron and for system back alike.
    val reveal = rememberLiquidReveal(open = env.nav.playerOpen)
    val fullyOpen by remember(reveal) { derivedStateOf { reveal.progress.value >= 1f } }
    var tabIndex by rememberSaveable { mutableIntStateOf(0) }
    val tab = PlayerTab.entries[tabIndex.coerceIn(0, PlayerTab.entries.lastIndex)]

    Box(
        Modifier
            .fillMaxSize()
            .liquidReveal(reveal, { env.nav.playerOrigin }),
    ) {
        Box(
            Modifier
                .fillMaxSize()
                .liquidContentAlpha(reveal)
                .background(Orbit.Void)
                // Swallow touches so nothing underneath the overlay reacts.
                .pointerInput(Unit) { detectTapGestures { } },
        ) {
            if (track == null) return@Box
            PlayerBackdrop(coverPath = track.thumbPath ?: track.fullPath, fullyOpen = fullyOpen)
            BoxWithConstraints(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)) {
                val onTab: (PlayerTab) -> Unit = { tabIndex = it.ordinal }
                val queueLen = state?.queueLen ?: 0u
                if (maxWidth > maxHeight) {
                    Row(Modifier.fillMaxSize()) {
                        Column(Modifier.weight(1f).fillMaxHeight()) {
                            Box(
                                Modifier.weight(1f).fillMaxWidth().padding(horizontal = Space.Gutter, vertical = Space.S),
                                contentAlignment = Alignment.Center,
                            ) { TabPanel(tab, track) }
                            TabChips(
                                tab,
                                queueLen,
                                onTab,
                                Modifier.fillMaxWidth().padding(horizontal = Space.Gutter, vertical = Space.Xs),
                            )
                        }
                        Column(Modifier.weight(1f).fillMaxHeight().verticalScroll(rememberScrollState())) {
                            TopBar(track, onClose)
                            Spacer(Modifier.height(Space.S))
                            Controls(track, state?.shuffle == true, state?.repeat ?: 0, state?.crossfadeApplies == true, compact = false, onClose = onClose)
                        }
                    }
                } else {
                    // Keep controls height stable across ART / LYRICS / QUEUE switches so changing tabs
                    // never forces a full-screen relayout jank.
                    val compactControls = maxHeight < ShortScreen
                    Column(Modifier.fillMaxSize()) {
                        TopBar(track, onClose)
                        Box(
                            Modifier.weight(1f).fillMaxWidth().padding(horizontal = Space.Xl, vertical = Space.Xs),
                            contentAlignment = Alignment.Center,
                        ) { TabPanel(tab, track) }
                        TabChips(
                            tab,
                            queueLen,
                            onTab,
                            Modifier.fillMaxWidth().padding(horizontal = Space.Xl),
                        )
                        Controls(track, state?.shuffle == true, state?.repeat ?: 0, state?.crossfadeApplies == true, compact = compactControls, onClose = onClose)
                    }
                }
            }
        }
    }
}

/**
 * Keeps visited tabs mounted and swaps visibility via GPU alpha only, eliminating the FFI/layout
 * hitch when toggling between `ART`, `LYRICS`, and `QUEUE`.
 */
@Composable
private fun TabPanel(tab: PlayerTab, track: TrackInfo) {
    val motion = LocalMotionEnabled.current
    var lyricsMounted by remember { mutableStateOf(false) }
    var queueMounted by remember { mutableStateOf(false) }
    LaunchedEffect(tab) {
        if (tab == PlayerTab.Lyrics) lyricsMounted = true
        if (tab == PlayerTab.Queue) queueMounted = true
    }

    val artAlpha by animateFloatAsState(
        targetValue = if (tab == PlayerTab.Art) 1f else 0f,
        animationSpec = orbitTween(Motion.TabFadeMs, motion),
        label = "tab-art",
    )
    val lyricsAlpha by animateFloatAsState(
        targetValue = if (tab == PlayerTab.Lyrics) 1f else 0f,
        animationSpec = orbitTween(Motion.TabFadeMs, motion),
        label = "tab-lyrics",
    )
    val queueAlpha by animateFloatAsState(
        targetValue = if (tab == PlayerTab.Queue) 1f else 0f,
        animationSpec = orbitTween(Motion.TabFadeMs, motion),
        label = "tab-queue",
    )

    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Box(
            Modifier.tabLayer(tab == PlayerTab.Art) { artAlpha },
            contentAlignment = Alignment.Center,
        ) {
            ArtContent(track, interactive = tab == PlayerTab.Art)
        }
        if (lyricsMounted) {
            Box(Modifier.tabLayer(tab == PlayerTab.Lyrics) { lyricsAlpha }) {
                LyricsPanel(track, Modifier.fillMaxSize(), active = tab == PlayerTab.Lyrics)
            }
        }
        if (queueMounted) {
            Box(Modifier.tabLayer(tab == PlayerTab.Queue) { queueAlpha }) {
                QueuePanel(Modifier.fillMaxSize(), active = tab == PlayerTab.Queue)
            }
        }
    }
}

/**
 * One stacked tab layer: the active one sits on top; hidden ones fade via the layer alpha only and
 * are removed from accessibility and from touch, so nothing under the visible tab reacts.
 */
private fun Modifier.tabLayer(active: Boolean, alpha: () -> Float): Modifier =
    this
        .fillMaxSize()
        .zIndex(if (active) 1f else 0f)
        .graphicsLayer { this.alpha = alpha() }
        .then(
            if (active) {
                Modifier
            } else {
                Modifier
                    .clearAndSetSemantics {}
                    .pointerInput(Unit) {
                        awaitPointerEventScope {
                            while (true) {
                                awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
                            }
                        }
                    }
            },
        )

@Composable
private fun ArtContent(track: TrackInfo, interactive: Boolean) {
    val env = LocalEnv.current
    val powerSave = LocalPowerSaveMode.current
    val motion = LocalMotionEnabled.current
    val density = LocalDensity.current
    val skipThreshold = with(density) { 64.dp.toPx() }
    val maxNudgePx = with(density) { 28.dp.toPx() }
    var dragX by remember { mutableFloatStateOf(0f) }
    val animNudge by animateFloatAsState(
        targetValue = if (powerSave || !motion) 0f else dragX,
        animationSpec = orbitTween(Motion.PressMs, motion),
        label = "art-swipe",
    )

    Box(
        Modifier
            .aspectRatio(1f)
            .fillMaxSize()
            .graphicsLayer { translationX = animNudge }
            .border(BorderStroke(Dimens.Stroke, Orbit.Line))
            .semantics {
                customActions = listOf(
                    CustomAccessibilityAction("Next track") { env.playback.next(); true },
                    CustomAccessibilityAction("Previous track") { env.playback.previous(); true },
                )
            }
            .then(
                if (interactive) {
                    Modifier.pointerInput(skipThreshold, maxNudgePx) {
                        var total = 0f
                        detectHorizontalDragGestures(
                            onDragStart = { total = 0f; dragX = 0f },
                            onDragCancel = { total = 0f; dragX = 0f },
                            onDragEnd = {
                                when {
                                    total <= -skipThreshold -> env.playback.next()
                                    total >= skipThreshold -> env.playback.previous()
                                }
                                total = 0f
                                dragX = 0f
                            },
                            onHorizontalDrag = { change, amount ->
                                change.consume()
                                total += amount
                                dragX = (total * 0.25f).coerceIn(-maxNudgePx, maxNudgePx)
                            },
                        )
                    }
                } else {
                    Modifier
                },
            ),
    ) {
        Cover(track.fullPath ?: track.thumbPath, Modifier.fillMaxSize(), bordered = false)
        CornerBrackets(Modifier.matchParentSize())
    }
}

/** Compact borderless `ART` / `LYRICS` / `QUEUE` switcher tucked directly below the cover. */
@Composable
private fun TabChips(selected: PlayerTab, queueLen: UInt, onSelect: (PlayerTab) -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier,
        horizontalArrangement = Arrangement.spacedBy(Dimens.IconL, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlayerTab.entries.forEach { t ->
            val active = t == selected
            val label = if (t == PlayerTab.Queue && queueLen > 0u) "QUEUE ($queueLen)" else t.label
            Row(
                Modifier
                    .heightIn(min = Dimens.Touch)
                    .pressable(pressedScale = 0.95f, role = Role.Tab) { onSelect(t) }
                    .padding(horizontal = Space.Xs, vertical = Space.Xs),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Space.Xs),
            ) {
                if (active) {
                    // Only the active chip reads the accent.
                    Box(
                        Modifier
                            .size(Space.Xs)
                            .clip(CircleShape)
                            .background(rememberReadableAccentOn(Orbit.Void)),
                    )
                }
                Text(
                    label,
                    color = if (active) Orbit.Text else Orbit.TextDim,
                    style = if (active) OrbitType.TabChipActive else OrbitType.TabChip,
                    maxLines = 1,
                )
            }
        }
    }
}

@Composable
private fun TopBar(track: TrackInfo, onClose: () -> Unit) {
    val env = LocalEnv.current
    val dismissThreshold = with(LocalDensity.current) { 56.dp.toPx() }
    var sheet by remember { mutableStateOf(false) }
    var audioSheet by remember { mutableStateOf(false) }
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .padding(horizontal = Space.Xs)
            .pointerInput(dismissThreshold) {
                var totalDown = 0f
                detectVerticalDragGestures(
                    onDragStart = { totalDown = 0f },
                    onDragCancel = { totalDown = 0f },
                    onDragEnd = {
                        if (totalDown >= dismissThreshold) onClose()
                        totalDown = 0f
                    },
                    onVerticalDrag = { change, amount ->
                        change.consume()
                        totalDown += amount
                    },
                )
            },
    ) {
        IconTap(
            OrbitIcons.Chevrondown,
            "Close player",
            Orbit.Text,
            modifier = Modifier.align(Alignment.CenterStart),
            onClick = onClose,
        )
        Column(
            Modifier
                .align(Alignment.Center)
                .padding(horizontal = 92.dp)
                .then(
                    if (track.album.isNotBlank()) {
                        Modifier.clickable {
                            onClose()
                            env.nav.push(Route.Album(track.album))
                        }
                    } else {
                        Modifier
                    },
                ),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                "NOW PLAYING",
                color = Orbit.TextDim,
                style = OrbitType.Section,
                maxLines = 1,
                softWrap = false,
                overflow = TextOverflow.Ellipsis,
            )
            if (track.album.isNotBlank()) {
                Text(
                    track.album,
                    color = Orbit.Text.withAlpha(Alpha.TextOnArt),
                    style = OrbitType.RowMeta,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Row(Modifier.align(Alignment.CenterEnd), verticalAlignment = Alignment.CenterVertically) {
            IconTap(OrbitIcons.Volume2, "Volume and equalizer", Orbit.Text) { audioSheet = true }
            IconTap(OrbitIcons.More, "Track actions", Orbit.Text) { sheet = true }
        }
    }
    if (sheet) TrackActionSheet(track, onDismiss = { sheet = false })
    if (audioSheet) VolumeEqSheet(onDismiss = { audioSheet = false })
}

@Composable
private fun Controls(
    track: TrackInfo,
    shuffle: Boolean,
    repeat: Int,
    crossfade: Boolean,
    compact: Boolean,
    onClose: () -> Unit,
) {
    val env = LocalEnv.current
    val accent = LocalAccent.current
    val readableAccent = rememberReadableAccentOn(Orbit.Void, accent)
    // Control state, not decoration: the real flag, not the lifecycle-gated LocalIsPlaying.
    val isPlaying by PlaybackHub.isPlaying.collectAsStateWithLifecycle()
    val usb by PlaybackHub.usb.collectAsStateWithLifecycle()
    val isLiked = track.id in LocalLikedIds.current

    Column(Modifier.fillMaxWidth().padding(horizontal = Space.Xl).padding(top = Space.S, bottom = Space.L)) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                Text(
                    track.title,
                    color = Orbit.Text,
                    style = if (compact) OrbitType.PlayerTitle else OrbitType.HeroTitle,
                    maxLines = if (compact) 1 else 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    track.artist,
                    modifier = if (track.artist.isNotBlank()) {
                        Modifier.clickable {
                            onClose()
                            env.nav.push(Route.Artist(track.artist))
                        }
                    } else {
                        Modifier
                    },
                    color = Orbit.TextDim,
                    style = if (compact) OrbitType.Body else OrbitType.PlayerArtist,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            IconTap(
                if (isLiked) OrbitIcons.Heartfilled else OrbitIcons.Heart,
                if (isLiked) "Unlike" else "Like",
                if (isLiked) Orbit.Danger else Orbit.TextDim,
            ) {
                env.collections.toggleLiked(track.id)
            }
        }
        val hiRes = track.bitsPerSample > 16 || track.sampleRate > 48_000
        val usbActive = usb as? UsbStatus.Active
        run {
            Spacer(Modifier.height(Space.S))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                if (usbActive != null) {
                    BitPerfectGlyph()
                } else {
                    Text("SHARED OUTPUT", color = Orbit.TextDim, style = OrbitType.Badge, maxLines = 1)
                }
                if (hiRes) Text("HI-RES", color = Orbit.Gold, style = OrbitType.Badge, maxLines = 1)
                if (track.qualityLabel.isNotBlank()) QualityPill(track.qualityLabel, accent = true)
                if (crossfade) Text("XFADE", color = Orbit.Teal, style = OrbitType.Badge, maxLines = 1)
            }
        }
        Spacer(Modifier.height(if (compact) Space.Xs else Space.S))
        SeekBar(onSeek = { env.playback.seekTo(it) }, fallbackDurationMs = (track.durationSecs * 1000).toLong())
        Spacer(Modifier.height(Space.S))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            IconTap(
                OrbitIcons.Shuffle, "Shuffle", if (shuffle) readableAccent else Orbit.TextDim,
                stateDescription = if (shuffle) "On" else "Off",
            ) { env.playback.toggleShuffle() }
            IconTap(OrbitIcons.Skipback, "Previous", Orbit.Text) { env.playback.previous() }
            HexButton(playing = isPlaying, onClick = { env.playback.togglePlay() }, size = 68.dp, accent = accent)
            IconTap(OrbitIcons.Skipforward, "Next", Orbit.Text) { env.playback.next() }
            val repeatTint = if (repeat != 0) readableAccent else Orbit.TextDim
            IconTap(
                if (repeat == 2) OrbitIcons.Repeat1 else OrbitIcons.Repeat,
                "Repeat",
                repeatTint,
                stateDescription = when (repeat) {
                    0 -> "Off"
                    2 -> "Repeat one"
                    else -> "Repeat all"
                },
            ) { env.playback.cycleRepeat() }
        }
    }
}

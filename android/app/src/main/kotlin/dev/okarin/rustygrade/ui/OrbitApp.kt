package dev.okarin.rustygrade.ui

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.LayoutDirection
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.currentStateAsState
import dev.okarin.rustygrade.ui.components.LocalIsPlaying
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import kotlin.math.roundToInt
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.orbitTween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.CollectionsViewModel
import dev.okarin.rustygrade.LibraryViewModel
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.MiniBar
import dev.okarin.rustygrade.ui.components.SpectrumBar
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.ScreenMove
import dev.okarin.rustygrade.ui.motion.orbitScreenTransform
import dev.okarin.rustygrade.ui.nav.NavState
import dev.okarin.rustygrade.ui.nav.encodeRoute
import dev.okarin.rustygrade.ui.nav.poppedRouteKeys
import dev.okarin.rustygrade.ui.nav.routeKeys
import dev.okarin.rustygrade.ui.nav.Route
import dev.okarin.rustygrade.ui.nav.Tab
import dev.okarin.rustygrade.ui.screens.AlbumDetailScreen
import dev.okarin.rustygrade.ui.screens.ArtistDetailScreen
import dev.okarin.rustygrade.ui.screens.LibraryScreen
import dev.okarin.rustygrade.ui.screens.NowPlayingScreen
import dev.okarin.rustygrade.ui.screens.PlaylistDetailScreen
import dev.okarin.rustygrade.ui.screens.PlaylistsScreen
import dev.okarin.rustygrade.ui.screens.SearchScreen
import dev.okarin.rustygrade.ui.screens.SettingsScreen
import dev.okarin.rustygrade.ui.screens.SmartPlaylistEditorScreen
import dev.okarin.rustygrade.ui.screens.TagEditorScreen
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitTheme
import dev.okarin.rustygrade.ui.theme.animatedAccent
import uniffi.rustygrade_ffi.RustygradeEngine

/** The app shell: current screen, persistent mini-bar, bottom navigation, full-screen player overlay. */
@Composable
fun OrbitApp(
    engine: RustygradeEngine,
    library: LibraryViewModel,
    collections: CollectionsViewModel,
    onFolderPicked: (Uri) -> Unit,
) {
    val nav = androidx.compose.runtime.saveable.rememberSaveable(saver = NavState.Saver) { NavState() }
    val actions = rememberScanActions(library, onFolderPicked)
    val env = remember(engine, library, collections, nav, actions) {
        UiEnv(engine, (library.getApplication<android.app.Application>() as dev.okarin.rustygrade.RustygradeApp).player, library, collections, nav, actions.scan, actions.addFolder)
    }
    val target by library.accent.collectAsStateWithLifecycle()
    val accent by animatedAccent(target.accent)
    val glow by animatedAccent(target.glow)

    BackHandler(enabled = nav.playerOpen || nav.canPop || nav.tab != Tab.Library) { nav.back() }

    val playerState = PlaybackHub.state.collectAsStateWithLifecycle()
    val nowPlayingId by remember { derivedStateOf { playerState.value?.current?.id } }
    val hasTrack = nowPlayingId != null
    val isPlaying by PlaybackHub.isPlaying.collectAsStateWithLifecycle()
    val lifecycleState by LocalLifecycleOwner.current.lifecycle.currentStateAsState()
    val isResumed = lifecycleState.isAtLeast(Lifecycle.State.RESUMED)
    val powerSave = rememberPowerSaveMode()
    val motionEnabled = !powerSave && dev.okarin.rustygrade.ui.theme.rememberSystemAnimationsOn()
    val likedIds by collections.liked.collectAsStateWithLifecycle()
    val settings by library.settings.collectAsStateWithLifecycle()
    val collectionsRefreshTick by library.collectionsRefreshTick.collectAsStateWithLifecycle()
    LaunchedEffect(collectionsRefreshTick) {
        if (collectionsRefreshTick > 0) collections.reload()
    }
    LaunchedEffect(settings?.scanOnStartup) {
        if (settings?.scanOnStartup == true && LibraryViewModel.claimStartupScan()) actions.scan()
    }

    OrbitTheme(accent, glow) {
        CompositionLocalProvider(
            LocalEnv provides env,
            dev.okarin.rustygrade.ui.components.LocalNowPlayingId provides nowPlayingId,
            dev.okarin.rustygrade.ui.components.LocalLikedIds provides likedIds,
            LocalPowerSaveMode provides powerSave,
            // Shell pulses (rows, mini-bar marquee) stop while backgrounded or covered by the player.
            LocalIsPlaying provides (isPlaying && isResumed && !nav.playerOpen),
            dev.okarin.rustygrade.ui.theme.LocalMotionEnabled provides motionEnabled,
        ) {
            // Height of the mini-bar + bottom navigation (incl. the navigation-bar inset). The keyboard
            // overlaps this chrome rather than pushing it up, so the content must only pad for the
            // part of the keyboard that really covers it (screens apply `imePadding()` themselves).
            var chromeHeightPx by remember { mutableIntStateOf(0) }
            val chromeHeight = with(LocalDensity.current) { chromeHeightPx.toDp() }
            // While an overlay is visibly open, the shell underneath must not be reachable by TalkBack
            // or Switch Access.
            val overlayOpen = nav.playerOpen
            Box(Modifier.fillMaxSize().background(Orbit.Void)) {
                Column(
                    Modifier
                        .fillMaxSize()
                        .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top))
                        .then(if (overlayOpen) Modifier.clearAndSetSemantics {} else Modifier),
                ) {
                    SpectrumBar()
                    Box(
                        Modifier
                            .weight(1f)
                            .fillMaxWidth()
                            .consumeWindowInsets(PaddingValues(bottom = chromeHeight)),
                    ) { Content(nav) }
                    Column(Modifier.onSizeChanged { chromeHeightPx = it.height }) {
                        if (hasTrack) MiniBar(onExpand = { nav.playerOpen = true })
                        // The tag editor is a focused sub-flow: switching tabs mid-save would dispose it.
                        if (nav.current !is Route.TagEditor) BottomNav(nav) else NavigationBarSpacer()
                    }
                }
                AnimatedVisibility(
                    visible = nav.playerOpen,
                    // The liquid blot is the reveal; this only keeps the screen composed while it closes (420 ms).
                    enter = androidx.compose.animation.EnterTransition.None,
                    // Fully opaque for the whole close of the blot, then gone; at once when motion is off.
                    exit = if (motionEnabled) {
                        fadeOut(tween(durationMillis = 1, delayMillis = Motion.PlayerExitDelayMs))
                    } else {
                        ExitTransition.None
                    },
                ) {
                    CompositionLocalProvider(LocalIsPlaying provides (isPlaying && isResumed)) {
                        NowPlayingScreen(onClose = { nav.playerOpen = false })
                    }
                }
            }
        }
    }
}

@Composable
private fun Content(nav: NavState) {
    val route = nav.current
    // Each screen keeps its saved state (scroll position, text fields) while it is not shown, so
    // going back from an album returns to the same place in the library.
    val holder = androidx.compose.runtime.saveable.rememberSaveableStateHolder()
    // Saved state of detail screens that were popped is dropped; tab roots stay for tab switches.
    val live = nav.routeKeys()
    val seen = remember { HashSet<String>() }
    LaunchedEffect(live) {
        poppedRouteKeys(seen, live).forEach(holder::removeState)
        seen.clear()
        seen.addAll(live)
    }
    val motion = dev.okarin.rustygrade.ui.theme.LocalMotionEnabled.current
    AnimatedContent(
        targetState = ScreenKey(route, nav.tab, nav.depth),
        transitionSpec = {
            val move = when {
                initialState.tab != targetState.tab -> ScreenMove.Switch
                targetState.depth > initialState.depth -> ScreenMove.Push
                targetState.depth < initialState.depth -> ScreenMove.Pop
                else -> ScreenMove.Switch
            }
            orbitScreenTransform(move, motion)
        },
        contentKey = { it.saveKey },
        label = "screen",
    ) { key ->
        val r = key.route
        val tab = key.tab
        holder.SaveableStateProvider(key = key.saveKey) {
            when (r) {
                is Route.Album -> AlbumDetailScreen(r.key)
                is Route.Artist -> ArtistDetailScreen(r.name)
                is Route.Playlist -> PlaylistDetailScreen(r.kind)
                is Route.SmartEditor -> SmartPlaylistEditorScreen(r.id)
                is Route.TagEditor -> TagEditorScreen(r.target)
                null -> when (tab) {
                    Tab.Library -> LibraryScreen()
                    Tab.Playlists -> PlaylistsScreen()
                    Tab.Search -> SearchScreen()
                    Tab.Settings -> SettingsScreen()
                }
            }
        }
    }
}

/** What the content area shows: the [route] on top of [tab]'s stack, [depth] routes deep. */
private data class ScreenKey(val route: Route?, val tab: Tab, val depth: Int) {
    val saveKey: String get() = if (route == null) "tab:${tab.name}" else encodeRoute(route)
}

@Composable
private fun BottomNav(nav: NavState) {
    val accent = LocalAccent.current
    val activeIconTint = rememberReadableAccentOn(Orbit.PanelDeep, accent)
    val motion = LocalMotionEnabled.current
    val tabs = Tab.entries
    // One accent bar that slides to the selected tab (the desktop nav marker), instead of four
    // bars blinking on and off.
    val indicator by animateFloatAsState(
        targetValue = tabs.indexOf(nav.tab).toFloat(),
        animationSpec = orbitTween(Motion.ScreenMs, motion),
        label = "tab-indicator",
    )
    Column(Modifier.fillMaxWidth().background(Orbit.PanelDeep).navigationBarsPadding()) {
        Divider()
        val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
        BoxWithConstraints(Modifier.fillMaxWidth().height(Dimens.Rule)) {
            val slot = maxWidth / tabs.size
            Box(
                Modifier
                    // Read in the layout phase: the slide does not recompose the nav every frame.
                    .offset {
                        val x = (slot.toPx() * indicator).roundToInt()
                        IntOffset(if (rtl) -x else x, 0)
                    }
                    .width(slot)
                    .fillMaxHeight()
                    .padding(horizontal = Space.Xl)
                    .background(accent),
            )
        }
        Row(
            Modifier.fillMaxWidth().height(Dimens.NavBarHeight - Dimens.Rule).selectableGroup(),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            tabs.forEach { tab ->
                val active = nav.tab == tab
                val icon = when (tab) {
                    Tab.Library -> OrbitIcons.Disc
                    Tab.Playlists -> OrbitIcons.Listmusic
                    Tab.Search -> OrbitIcons.Search
                    Tab.Settings -> OrbitIcons.Sliders
                }
                val tint by animateColorAsState(
                    if (active) activeIconTint else Orbit.TextDim,
                    orbitTween(Motion.HoverMs, motion),
                    label = "tab-tint",
                )
                val labelColor by animateColorAsState(
                    if (active) Orbit.Text else Orbit.TextDim,
                    orbitTween(Motion.HoverMs, motion),
                    label = "tab-label",
                )
                val lift by animateFloatAsState(if (active) 1f else 0f, orbitTween(Motion.ScreenMs, motion), label = "tab-lift")
                Column(
                    Modifier.weight(1f).fillMaxSize().selectable(
                        selected = active,
                        role = Role.Tab,
                        onClick = { nav.selectTab(tab) },
                    ),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(Space.Xs, Alignment.CenterVertically),
                ) {
                    // The label right below names the tab; a second description would be read twice.
                    Icon(
                        icon,
                        null,
                        tint = tint,
                        modifier = Modifier
                            .size(21.dp)
                            .graphicsLayer {
                                val s = 1f + 0.08f * lift
                                scaleX = s
                                scaleY = s
                                translationY = -Space.Hair.toPx() * lift
                            },
                    )
                    Text(
                        tab.label,
                        color = labelColor,
                        style = if (active) OrbitType.NavLabelActive else OrbitType.NavLabel,
                        maxLines = 1,
                        softWrap = false,
                    )
                }
            }
        }
    }
}

/** Keeps the bottom navigation-bar inset (and its backdrop) when the navigation bar itself is hidden. */
@Composable
private fun NavigationBarSpacer() {
    Box(Modifier.fillMaxWidth().background(Orbit.PanelDeep).navigationBarsPadding())
}

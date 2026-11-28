package dev.okarin.rustygrade.ui.nav

import androidx.compose.runtime.Stable
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** The four bottom-navigation tabs. */
enum class Tab(val label: String) {
    Library("LIBRARY"),
    Playlists("PLAYLISTS"),
    Search("SEARCH"),
    Settings("SETTINGS"),
}

/** Which collection a playlist screen shows. */
sealed interface PlaylistKind {
    data object Favorites : PlaylistKind
    data class Custom(val id: Long) : PlaylistKind
    data class Smart(val id: Long) : PlaylistKind
}

/** Screens pushed on top of a tab's root. */
sealed interface Route {
    data class Album(val key: String) : Route
    data class Artist(val name: String) : Route
    data class Playlist(val kind: PlaylistKind) : Route

    /** `id == 0L` creates a new smart playlist. */
    data class SmartEditor(val id: Long) : Route

    /** Edit the tags of one track or of every track of an album. */
    data class TagEditor(val target: TagTarget) : Route
}

sealed interface TagTarget {
    data class Track(val id: String) : TagTarget
    data class Album(val key: String) : TagTarget
}

/**
 * Navigation state: the selected [Tab] plus a stack of [Route]s above it. Selecting a tab resets
 * the stack (tapping the tab you are on pops back to its root). Hoisted so it survives
 * recomposition; the full-screen player is an overlay flag, not a route.
 */
@Stable
class NavState(initial: Tab = Tab.Library) {
    var tab: Tab by mutableStateOf(initial)
        private set

    private val stack = mutableStateListOf<Route>()

    val current: Route? get() = stack.lastOrNull()
    val depth: Int get() = stack.size
    val canPop: Boolean get() = stack.isNotEmpty()

    /** Whether the full-screen player is open on top of everything. */
    var playerOpen: Boolean by mutableStateOf(false)

    /** Where the full-screen player grows from (window position of the mini-bar's play button). */
    var playerOrigin: androidx.compose.ui.geometry.Offset? by mutableStateOf(null)

    /** One-shot request to reset Library scroll to top when the Home (LIBRARY) tab is selected. */
    var libraryScrollToTopPending: Boolean by mutableStateOf(false)
        private set

    fun consumeLibraryScrollToTop(): Boolean {
        if (!libraryScrollToTopPending) return false
        libraryScrollToTopPending = false
        return true
    }

    fun selectTab(t: Tab) {
        if (t == Tab.Library) libraryScrollToTopPending = true
        tab = t
        stack.clear()
    }

    fun push(route: Route) {
        if (stack.lastOrNull() != route) stack.add(route)
    }

    /** Pops one screen. Returns `false` when already at a tab root. */
    fun pop(): Boolean {
        if (stack.isEmpty()) return false
        stack.removeAt(stack.lastIndex)
        return true
    }

    fun popToRoot() = stack.clear()

    /** Back button behaviour: player first, then detail screens, then back to the first tab. */
    fun back(): Boolean = when {
        playerOpen -> { playerOpen = false; true }
        pop() -> true
        tab != Tab.Library -> { tab = Tab.Library; true }
        else -> false
    }

    /** The detail stack, bottom first (for saving and for per-screen state keys). */
    fun routes(): List<Route> = stack.toList()

    companion object {
        /** Keeps the tab, the detail stack and overlay flags across rotation and process death. */
        val Saver: Saver<NavState, List<String>> = Saver(
            save = { nav ->
                listOf(nav.tab.name, nav.playerOpen.toString()) + nav.routes().map(::encodeRoute)
            },
            restore = { saved ->
                val tab = Tab.entries.firstOrNull { it.name == saved.getOrNull(0) } ?: Tab.Library
                NavState(tab).also { nav ->
                    saved.drop(2).mapNotNull(::decodeRoute).forEach(nav::push)
                    nav.playerOpen = saved.getOrNull(1) == "true"
                }
            },
        )
    }
}

/** `kind|payload`; the payload may itself contain `|`, only the first one separates. */
fun encodeRoute(route: Route): String = when (route) {
    is Route.Album -> "album|${route.key}"
    is Route.Artist -> "artist|${route.name}"
    is Route.Playlist -> when (val k = route.kind) {
        PlaylistKind.Favorites -> "playlist|fav"
        is PlaylistKind.Custom -> "playlist|custom:${k.id}"
        is PlaylistKind.Smart -> "playlist|smart:${k.id}"
    }
    is Route.SmartEditor -> "smart|${route.id}"
    is Route.TagEditor -> when (val t = route.target) {
        is TagTarget.Track -> "tags|track:${t.id}"
        is TagTarget.Album -> "tags|album:${t.key}"
    }
}

fun decodeRoute(text: String): Route? {
    val payload = text.substringAfter('|', "")
    return when (text.substringBefore('|')) {
        "album" -> Route.Album(payload)
        "artist" -> Route.Artist(payload)
        "playlist" -> when {
            payload == "fav" -> Route.Playlist(PlaylistKind.Favorites)
            payload.startsWith("custom:") -> payload.removePrefix("custom:").toLongOrNull()
                ?.let { Route.Playlist(PlaylistKind.Custom(it)) }
            payload.startsWith("smart:") -> payload.removePrefix("smart:").toLongOrNull()
                ?.let { Route.Playlist(PlaylistKind.Smart(it)) }
            else -> null
        }
        "smart" -> payload.toLongOrNull()?.let { Route.SmartEditor(it) }
        "tags" -> when {
            payload.startsWith("track:") -> Route.TagEditor(TagTarget.Track(payload.removePrefix("track:")))
            payload.startsWith("album:") -> Route.TagEditor(TagTarget.Album(payload.removePrefix("album:")))
            else -> null
        }
        else -> null
    }
}

/** The saveable-state keys of the detail screens on the stack (tab roots are keyed separately). */
fun NavState.routeKeys(): Set<String> = routes().mapTo(LinkedHashSet(), ::encodeRoute)

/** Keys that were live before and are not any more: the saved UI state of those screens can go. */
fun poppedRouteKeys(previous: Set<String>, live: Set<String>): Set<String> = previous - live

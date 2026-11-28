package dev.okarin.rustygrade.ui.player

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.UiEnv
import dev.okarin.rustygrade.ui.components.ConfirmDialog
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.EmptyMessage
import dev.okarin.rustygrade.ui.components.LocalIsPlaying
import dev.okarin.rustygrade.ui.components.IconTap
import dev.okarin.rustygrade.ui.components.formatDuration
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.motion.PulseDot
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.first
import uniffi.rustygrade_ffi.PlayerState
import uniffi.rustygrade_ffi.TrackInfo
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

private const val PAGE = 100
private const val LEAD = 20
private const val NEAR_EDGE = 8
private val RowHeight = 60.dp

/** A loaded slice `[start, start + items.size)` of the play order. */
private class QueueWindow(val start: Int, val items: List<TrackInfo>)

private fun rowKey(position: Int, track: TrackInfo): String = "$position:${track.id}"

/**
 * The play order around the current track, paged (with shuffle the queue is the whole library, so
 * only a window of it is ever loaded). Tap jumps, the X removes, and a long press drags a row to a
 * new position; the move is committed once, on release.
 *
 * While not [active] (another player tab is showing) the panel stays mounted but stops reloading
 * on queue changes and stops its pulse; it catches up as soon as it is shown again.
 */
@Composable
fun QueuePanel(modifier: Modifier = Modifier, active: Boolean = true) {
    val state by PlaybackHub.state.collectAsStateWithLifecycle()
    val s = state
    if (s == null || s.queueLen == 0u) {
        Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) { EmptyMessage("QUEUE IS EMPTY") }
    } else {
        QueueContent(s, active, modifier)
    }
}

@Composable
private fun QueueContent(s: PlayerState, active: Boolean, modifier: Modifier) {
    val env = LocalEnv.current
    val total = s.queueLen.toInt()
    val revision = s.revision

    val latestTotal by rememberUpdatedState(total)
    val latestIndex by rememberUpdatedState(s.index)
    val latestRevision by rememberUpdatedState(revision)

    var window by remember { mutableStateOf(QueueWindow(0, emptyList())) }
    // The current index after a just-committed drag, until the real state catches up.
    var optimisticIndex by remember { mutableStateOf<Int?>(null) }
    var ready by remember { mutableStateOf(false) }
    var showClear by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()

    // Moves the item at [from] to [to] (both play-order positions) and mirrors it in the loaded
    // window right away; shared by drag-and-drop and the accessibility "move" actions.
    fun commitMove(from: Int, to: Int) {
        env.playback.moveItem(from, to)
        val w = window
        val lf = from - w.start
        val lt = to - w.start
        if (lf in w.items.indices && lt in w.items.indices) {
            val list = w.items.toMutableList()
            list.add(lt, list.removeAt(lf))
            window = QueueWindow(w.start, list)
            val cur = optimisticIndex ?: latestIndex
            optimisticIndex = when {
                from == cur -> to
                from < cur && to >= cur -> cur - 1
                from > cur && to <= cur -> cur + 1
                else -> cur
            }
        }
    }

    val drag = rememberDragReorderState(
        listState = listState,
        baseIndex = { window.start },
        onCommit = { from, to -> commitMove(from, to) },
    )

    // (Re)load on every queue or track change, keeping what the user is looking at.
    LaunchedEffect(revision, active) {
        if (!active) return@LaunchedEffect
        // The positions the drag started from no longer mean the same tracks.
        if (drag.isDragging) drag.cancel()
        val cur = window
        val t = latestTotal
        var start = 0
        var end = 0
        if (cur.items.isNotEmpty() && cur.start < t) {
            val first = listState.firstVisibleItemIndex
            val last = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: first
            start = max(cur.start, cur.start + first - PAGE / 2)
            end = min(min(t, cur.start + cur.items.size), cur.start + last + PAGE / 2 + 1)
        }
        if (end <= start) {
            start = max(0, min(latestIndex - LEAD, t - 1))
            end = min(t, start + PAGE)
        }
        val page = loadPage(env, start, end - start)
        if (!page.isNullOrEmpty()) {
            window = QueueWindow(start, page)
            optimisticIndex = null
        }
    }

    // First paint: put the current track near the top once the first page is laid out.
    LaunchedEffect(Unit) {
        snapshotFlow { listState.layoutInfo.totalItemsCount }.first { it > 0 }
        val w = window
        listState.scrollToItem(max(0, latestIndex - w.start - 1))
        ready = true
    }

    // Extend the window when the user scrolls near either edge.
    LaunchedEffect(listState) {
        snapshotFlow {
            if (!ready) {
                null
            } else {
                val first = listState.firstVisibleItemIndex
                val last = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: first
                Triple(window, first, last)
            }
        }.collect { snap ->
            if (snap == null) return@collect
            val (w, first, last) = snap
            if (w.items.isEmpty()) return@collect
            val rev = latestRevision
            if (first < NEAR_EDGE && w.start > 0) {
                val newStart = max(0, w.start - PAGE)
                val page = loadPage(env, newStart, w.start - newStart)
                val now = window
                if (!page.isNullOrEmpty() && rev == latestRevision && now.start == w.start) {
                    window = QueueWindow(newStart, page + now.items)
                }
            } else if (last >= w.items.size - NEAR_EDGE && w.start + w.items.size < latestTotal) {
                val end = w.start + w.items.size
                val page = loadPage(env, end, min(PAGE, latestTotal - end))
                val now = window
                if (!page.isNullOrEmpty() && rev == latestRevision && now.start == w.start && now.items.size == w.items.size) {
                    window = QueueWindow(w.start, now.items + page)
                }
            }
        }
    }

    // Follow the playing track when it moves off screen while the user was looking at it.
    var lastIndex by remember { mutableIntStateOf(s.index) }
    LaunchedEffect(s.index) {
        val prev = lastIndex
        lastIndex = s.index
        if (prev == s.index || drag.isDragging) return@LaunchedEffect
        val w = window
        val visible = listState.layoutInfo.visibleItemsInfo
        val prevShown = visible.any { w.start + it.index == prev }
        val nowShown = visible.any { w.start + it.index == s.index }
        val local = s.index - w.start
        if (prevShown && !nowShown && local in w.items.indices) listState.scrollToItem(max(0, local - 1))
    }

    val w = window
    val currentIndex = optimisticIndex ?: s.index
    val handLen = s.handLen.toInt()

    Column(modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(bottom = Space.S),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.S),
        ) {
            Column(Modifier.weight(1f)) {
                Text(
                    "QUEUE · ${currentIndex + 1} / $total",
                    color = Orbit.TextDim,
                    style = OrbitType.Section,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (handLen > 0) {
                    Text("$handLen QUEUED BY HAND", color = Orbit.Gold, style = OrbitType.Badge, maxLines = 1)
                }
            }
            IconTap(OrbitIcons.Trash, "Clear queue", Orbit.Danger, iconSize = Dimens.IconM) { showClear = true }
        }

        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .background(Orbit.Void.withAlpha(Alpha.Scrim))
                .border(BorderStroke(Dimens.Stroke, Orbit.LineSoft)),
        ) {
            if (w.items.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("LOADING…", color = Orbit.TextDim, style = OrbitType.Section)
                }
            } else {
                LazyColumn(state = listState, modifier = Modifier.fillMaxSize().dragReorder(drag)) {
                    itemsIndexed(w.items, key = { i, t -> rowKey(w.start + i, t) }, contentType = { _, _ -> "queue-row" }) { i, track ->
                        val position = w.start + i
                        val key = rowKey(position, track)
                        val dragging = drag.isDragging
                        val indicator = when {
                            !dragging || position != drag.targetIndex || position == drag.fromIndex -> 0
                            drag.targetIndex < drag.fromIndex -> -1
                            else -> 1
                        }
                        QueueRow(
                            position = position,
                            track = track,
                            isCurrent = position == currentIndex,
                            isHand = position > currentIndex && position <= currentIndex + handLen,
                            indicator = indicator,
                            placeholder = dragging && drag.draggedKey == key,
                            ghost = false,
                            onClick = { env.playback.jumpTo(position) },
                            onRemove = { env.playback.removeAt(position) },
                            onMoveBy = { delta ->
                                val to = position + delta
                                if (to in 0 until total) commitMove(position, to)
                            },
                            canMoveUp = position > 0,
                            canMoveDown = position < total - 1,
                            pulse = active,
                        )
                    }
                }
                if (drag.isDragging) {
                    val draggedKey = drag.draggedKey
                    val ghostTrack = remember(draggedKey, w) { w.items.getOrNull(drag.fromIndex - w.start) }
                    if (ghostTrack != null) {
                        QueueRow(
                            position = drag.fromIndex,
                            track = ghostTrack,
                            isCurrent = drag.fromIndex == currentIndex,
                            isHand = false,
                            indicator = 0,
                            placeholder = false,
                            ghost = true,
                            onClick = {},
                            onRemove = {},
                            modifier = Modifier.offset { IntOffset(0, drag.ghostTop.roundToInt()) },
                        )
                    }
                }
            }
        }
    }

    if (showClear) {
        ConfirmDialog(
            message = "Clear the whole queue? Playback will stop.",
            confirmLabel = "CLEAR",
            onConfirm = {
                showClear = false
                env.playback.clearQueue()
            },
            onDismiss = { showClear = false },
        )
    }
}

/** One queue entry. [indicator]: -1 draws the insertion marker on the top edge, 1 on the bottom edge. */
@Composable
private fun QueueRow(
    position: Int,
    track: TrackInfo,
    isCurrent: Boolean,
    isHand: Boolean,
    indicator: Int,
    placeholder: Boolean,
    ghost: Boolean,
    onClick: () -> Unit,
    onRemove: () -> Unit,
    modifier: Modifier = Modifier,
    onMoveBy: (delta: Int) -> Unit = {},
    canMoveUp: Boolean = false,
    canMoveDown: Boolean = false,
    pulse: Boolean = true,
) {
    // Only rows that draw the accent read it, so an accent fade does not recompose the whole queue.
    val accent = if (isCurrent || ghost || indicator != 0) rememberReadableAccentOn(Orbit.Void) else Orbit.Text
    Row(
        modifier
            .fillMaxWidth()
            // Reordering by dragging has no TalkBack equivalent: offer it as actions.
            .then(
                if (ghost) Modifier else Modifier.semantics {
                    customActions = buildList {
                        if (canMoveUp) add(CustomAccessibilityAction("Move up") { onMoveBy(-1); true })
                        if (canMoveDown) add(CustomAccessibilityAction("Move down") { onMoveBy(1); true })
                    }
                },
            )
            .height(RowHeight)
            .alpha(if (placeholder) Alpha.Faint else 1f)
            .then(if (ghost) Modifier.background(Orbit.Raised).border(Dimens.Stroke, accent) else Modifier)
            .drawBehind {
                if (isCurrent) drawRect(accent.withAlpha(Alpha.Tint))
                val hair = Dimens.Stroke.toPx()
                drawRect(Orbit.LineSoft, topLeft = Offset(0f, size.height - hair), size = Size(size.width, hair))
                if (isHand) drawRect(Orbit.Gold, size = Size(Dimens.Rule.toPx(), size.height))
                val bar = Dimens.Rule.toPx()
                if (indicator < 0) drawRect(accent, size = Size(size.width, bar))
                else if (indicator > 0) drawRect(accent, topLeft = Offset(0f, size.height - bar), size = Size(size.width, bar))
            },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier
                .weight(1f)
                .fillMaxHeight()
                .pressable(enabled = !ghost, pressedScale = 0.985f, onClick = onClick)
                .padding(start = Dimens.RowPadV),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Dimens.RowPadV),
        ) {
            if (isCurrent) {
                // The pulsing dot replaces the position, like TrackRow; it still reads as the position.
                Box(
                    Modifier.widthIn(min = Space.Xxl).semantics { contentDescription = "Now playing, ${position + 1}" },
                    contentAlignment = Alignment.CenterEnd,
                ) {
                    PulseDot(accent, size = 5.dp, active = LocalIsPlaying.current && pulse && !ghost)
                }
            } else {
                Text(
                    "${position + 1}",
                    modifier = Modifier.widthIn(min = Space.Xxl),
                    color = Orbit.TextDim,
                    style = OrbitType.Numeric,
                    textAlign = TextAlign.End,
                    maxLines = 1,
                )
            }
            Cover(track.thumbPath, size = 42.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                Text(
                    track.title,
                    color = if (isCurrent) accent else Orbit.Text,
                    style = if (isCurrent) OrbitType.RowTitle else OrbitType.RowTitleSoft,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    track.artist,
                    color = Orbit.TextDim,
                    style = OrbitType.RowMeta,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(formatDuration(track.durationSecs), color = Orbit.TextDim, style = OrbitType.Numeric, maxLines = 1)
        }
        Icon(OrbitIcons.Grip, "Hold and drag to reorder", tint = Orbit.TextDim, modifier = Modifier.size(Space.Xxxl).padding(7.dp))
        Box(
            Modifier.size(Dimens.Touch).clickable(enabled = !ghost, onClick = onRemove),
            contentAlignment = Alignment.Center,
        ) {
            Icon(OrbitIcons.X, "Remove from queue", tint = Orbit.TextDim, modifier = Modifier.size(Dimens.IconS))
        }
    }
}

/** One page of the play order, or null when the engine failed. */
private suspend fun loadPage(env: UiEnv, start: Int, count: Int): List<TrackInfo>? {
    if (count <= 0 || start < 0) return emptyList()
    return try {
        io { env.player.queuePage(start.toUInt(), count.toUInt()) }
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        null
    }
}

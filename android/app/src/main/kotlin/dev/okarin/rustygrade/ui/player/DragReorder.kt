package dev.okarin.rustygrade.ui.player

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalHapticFeedback
import kotlinx.coroutines.flow.collectLatest
import kotlin.math.max

/**
 * Long-press-and-drag reordering for a [LazyListState] list, driven from the list itself (not from
 * the rows) so the gesture survives the dragged row scrolling out of composition.
 *
 * Nothing moves while hovering: the dragged row is drawn as a ghost at [ghostTop] (viewport pixels)
 * and an insertion marker sits on [targetIndex]; the caller's `onCommit(from, to)` runs exactly once,
 * on release. Indices are absolute (local list index plus the `baseIndex` supplied by the caller).
 */
@Stable
class DragReorderState internal constructor(
    private val listState: LazyListState,
    private val base: () -> Int,
    private val itemIndexOffset: Int,
    private val canDrag: (Any) -> Boolean,
    private val onCommit: (from: Int, to: Int) -> Unit,
    private val onPickUp: () -> Unit,
) {
    /** The LazyColumn key of the row being dragged, or null when idle. */
    var draggedKey: Any? by mutableStateOf(null)
        private set

    /** Absolute index the dragged row started from (-1 when idle). */
    var fromIndex by mutableIntStateOf(-1)
        private set

    /** Absolute index the row would land on if released now (-1 when idle). */
    var targetIndex by mutableIntStateOf(-1)
        private set

    /** Top edge of the ghost row in list-viewport pixels. */
    var ghostTop by mutableFloatStateOf(0f)
        private set

    /** Height of the dragged row in pixels. */
    var itemSize by mutableFloatStateOf(0f)
        private set

    val isDragging: Boolean get() = draggedKey != null

    private var fingerY = 0f
    private var grab = 0f

    internal fun begin(y: Float): Boolean {
        val info = listState.layoutInfo.visibleItemsInfo
            .firstOrNull { y >= it.offset && y < it.offset + it.size } ?: return false
        if (!canDrag(info.key)) return false
        val localIndex = info.index - itemIndexOffset
        if (localIndex < 0) return false
        grab = y - info.offset
        fingerY = y
        itemSize = info.size.toFloat()
        fromIndex = base() + localIndex
        targetIndex = fromIndex
        draggedKey = info.key
        placeGhost()
        updateTarget()
        onPickUp()
        return true
    }

    internal fun dragBy(dy: Float) {
        if (!isDragging) return
        fingerY += dy
        placeGhost()
        updateTarget()
    }

    internal fun finish(commit: Boolean) {
        val from = fromIndex
        val to = targetIndex
        cancel()
        if (commit && from >= 0 && to >= 0 && from != to) onCommit(from, to)
    }

    fun cancel() {
        draggedKey = null
        fromIndex = -1
        targetIndex = -1
        fingerY = 0f
    }

    /** Scrolls the list while the finger rests near its top or bottom edge. Called once per frame. */
    internal suspend fun autoScroll() {
        if (!isDragging) return
        val info = listState.layoutInfo
        val viewport = (info.viewportEndOffset - info.viewportStartOffset).toFloat()
        val zone = itemSize * 1.25f
        if (zone <= 0f) return
        val ramp = when {
            fingerY < zone -> -((zone - fingerY) / zone).coerceIn(0f, 1f)
            fingerY > viewport - zone -> ((fingerY - (viewport - zone)) / zone).coerceIn(0f, 1f)
            else -> 0f
        }
        if (ramp == 0f) return
        listState.scrollBy(ramp * MAX_SCROLL_PX_PER_FRAME)
        placeGhost()
        updateTarget()
    }

    private fun placeGhost() {
        val info = listState.layoutInfo
        val viewport = (info.viewportEndOffset - info.viewportStartOffset).toFloat()
        ghostTop = (fingerY - grab).coerceIn(0f, max(0f, viewport - itemSize))
    }

    private fun updateTarget() {
        val visible = listState.layoutInfo.visibleItemsInfo
            .filter { canDrag(it.key) && it.index >= itemIndexOffset }
        if (visible.isEmpty()) return
        val center = ghostTop + itemSize / 2f
        val hit = visible.firstOrNull { center >= it.offset && center < it.offset + it.size }
            ?: if (center < visible.first().offset) visible.first() else visible.last()
        targetIndex = base() + (hit.index - itemIndexOffset)
    }

    private companion object {
        const val MAX_SCROLL_PX_PER_FRAME = 28f
    }
}

/**
 * Creates the drag state for [listState]. [baseIndex] maps a local list index to the absolute index
 * (for a paged window: the window start); [itemIndexOffset] skips fixed header items before rows;
 * [canDragKey] filters out non-row keys; [onCommit] receives absolute `from`/`to` on release.
 */
@Composable
fun rememberDragReorderState(
    listState: LazyListState,
    baseIndex: () -> Int,
    itemIndexOffset: Int = 0,
    canDragKey: (Any) -> Boolean = { true },
    onCommit: (from: Int, to: Int) -> Unit,
): DragReorderState {
    val haptics = LocalHapticFeedback.current
    val baseState = rememberUpdatedState(baseIndex)
    val canDragState = rememberUpdatedState(canDragKey)
    val commitState = rememberUpdatedState(onCommit)
    val hapticsState = rememberUpdatedState(haptics)
    val state = remember(listState, itemIndexOffset) {
        DragReorderState(
            listState = listState,
            base = { baseState.value() },
            itemIndexOffset = itemIndexOffset,
            canDrag = { canDragState.value(it) },
            onCommit = { f, t -> commitState.value(f, t) },
            onPickUp = { hapticsState.value.performHapticFeedback(HapticFeedbackType.LongPress) },
        )
    }
    // Frame-driven only while a drag is in progress.
    LaunchedEffect(state) {
        snapshotFlow { state.isDragging }.collectLatest { dragging ->
            while (dragging) {
                androidx.compose.runtime.withFrameNanos { }
                state.autoScroll()
            }
        }
    }
    return state
}

/**
 * Attach to the LazyColumn: a long press on any row (without moving past touch slop first) picks the
 * row up; moving then drags it, releasing commits. Ordinary taps and scrolls are untouched.
 */
fun Modifier.dragReorder(state: DragReorderState, enabled: Boolean = true): Modifier =
    if (!enabled) this else pointerInput(state) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        val slop = viewConfiguration.touchSlop
        // null = the long-press timeout elapsed with the finger still down and still; false = abandoned.
        val abandoned: Boolean? = withTimeoutOrNull(viewConfiguration.longPressTimeoutMillis) {
            var waiting = true
            while (waiting) {
                val change = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id }
                if (change == null || !change.pressed || change.isConsumed ||
                    (change.position - down.position).getDistance() > slop
                ) {
                    waiting = false
                }
            }
            false
        }
        if (abandoned != null) return@awaitEachGesture
        if (!state.begin(down.position.y)) return@awaitEachGesture
        try {
            var commit = true
            while (true) {
                val change = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id }
                if (change == null) {
                    commit = false
                    break
                }
                val dy = change.positionChange().y
                change.consume()
                if (!change.pressed) break
                state.dragBy(dy)
            }
            state.finish(commit)
        } finally {
            state.cancel()
        }
    }
}

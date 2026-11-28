package dev.okarin.rustygrade.ui.motion

import android.os.SystemClock
import androidx.compose.animation.core.Animatable
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.node.LayoutModifierNode
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.platform.InspectorInfo
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.orbitTween
import kotlinx.coroutines.launch

/**
 * Plays the desktop page entrance (`EnterSpec`: fade in while rising a few dp, cubic-out) once per
 * item when a screen opens, as a short cascade. Items that compose later (scrolled into view)
 * just appear: replaying on scroll would make a 100-album grid shimmer on every fling.
 *
 * Remember one scope per screen; the cascade window closes [windowMs] after the first item.
 */
@Stable
class EnterScope internal constructor(private val windowMs: Long) {
    private val seen = HashSet<Any>()
    private var firstAt = -1L
    private var lastAt = 0L
    private var burst = 0

    /** The cascade slot of [key], or -1 when it must not animate (already shown, or window closed). */
    internal fun claim(key: Any): Int {
        if (!seen.add(key)) return -1
        val now = SystemClock.uptimeMillis()
        if (firstAt < 0) firstAt = now
        if (now - firstAt > windowMs) return -1
        // A gap longer than ABSENCE starts a new burst (e.g. results arriving after a load).
        if (now - lastAt > Motion.AbsenceMs) burst = 0
        lastAt = now
        return burst++
    }
}

/** One cascade per screen; a new [key] (e.g. a new search query) starts a fresh cascade. */
@Composable
fun rememberEnterScope(key: Any? = null, windowMs: Long = Motion.EnterWindowMs): EnterScope =
    remember(key) { EnterScope(windowMs) }

/**
 * Cascade entrance for an item identified by [key] inside [scope]. Items that must not animate
 * (motion off, already seen, window closed) get no node at all; the rest get a small layout node
 * that fades / lifts them once on attach.
 */
@Composable
fun Modifier.enterOnce(
    scope: EnterScope,
    key: Any,
    rise: Dp = Motion.PageEnterRise,
    durationMs: Int = Motion.PageEnterMs,
): Modifier {
    if (!LocalMotionEnabled.current) return this
    val slot = remember(scope, key) { scope.claim(key) }
    if (slot < 0) return this
    val risePx = with(LocalDensity.current) { rise.toPx() }
    val delayMs = minOf(slot, Motion.StaggerCap) * Motion.StaggerMs
    return this then EnterElement(key, risePx, durationMs, delayMs)
}

private data class EnterElement(
    val key: Any,
    val risePx: Float,
    val durationMs: Int,
    val delayMs: Int,
) : ModifierNodeElement<EnterNode>() {
    override fun create() = EnterNode(key, risePx, durationMs, delayMs)

    override fun update(node: EnterNode) {
        node.update(key, risePx, durationMs, delayMs)
    }

    override fun InspectorInfo.inspectableProperties() {
        name = "enterOnce"
        properties["key"] = key
        properties["durationMs"] = durationMs
        properties["delayMs"] = delayMs
    }
}

private class EnterNode(
    private var key: Any,
    private var risePx: Float,
    private var durationMs: Int,
    private var delayMs: Int,
) : Modifier.Node(), LayoutModifierNode {
    private val progress = Animatable(0f)

    /** Set once the entrance started, so a detach / re-attach of the same item never replays it. */
    private var played = false

    override fun onAttach() = play()

    fun update(key: Any, risePx: Float, durationMs: Int, delayMs: Int) {
        this.risePx = risePx
        this.durationMs = durationMs
        this.delayMs = delayMs
        // A node reused for another (newly claimed) item plays that item's entrance.
        if (this.key != key) {
            this.key = key
            played = false
            if (isAttached) play()
        }
    }

    private fun play() {
        val replay = played
        played = true
        coroutineScope.launch {
            if (replay) {
                progress.snapTo(1f)
            } else {
                progress.snapTo(0f)
                progress.animateTo(1f, orbitTween(durationMs, enabled = true, delayMs = delayMs))
            }
        }
    }

    override fun MeasureScope.measure(measurable: Measurable, constraints: Constraints): MeasureResult {
        val placeable = measurable.measure(constraints)
        return layout(placeable.width, placeable.height) {
            placeable.placeWithLayer(0, 0) {
                val p = progress.value
                alpha = p
                translationY = (1f - p) * risePx
            }
        }
    }
}

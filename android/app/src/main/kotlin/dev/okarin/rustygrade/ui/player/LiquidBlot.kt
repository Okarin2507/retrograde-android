package dev.okarin.rustygrade.ui.player

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.node.CompositionLocalConsumerModifierNode
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.GlobalPositionAwareModifierNode
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.node.currentValueOf
import androidx.compose.ui.platform.InspectorInfo
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalGlow
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.Orbit
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.random.Random

private const val BLOT_POINTS = 18

/** How much of the cover glow tints the void inside the blot. */
private const val BLOT_GLOW = 0.22f
private const val DROPLET_POINTS = 10
private const val TAU = 6.2831855f

/** Ease-out quart: quick initial spill, long soft spread. */
private val EaseOutQuart = Easing { t -> 1f - (1f - t).pow(4) }

/** Ease-in cubic: the blot draws back smoothly into the button. */
private val EaseInCubic = Easing { t -> t * t * t }

/** One of the four circles that run ahead of the blot's edge. */
internal class Droplet(val jitter: Float, val distance: Float, val size: Float, val phase: Float)

/**
 * The liquid reveal of the full-screen player: a single [progress] (0 = hidden, 1 = fully open)
 * that is animated only while opening (700 ms, ease-out) or closing (420 ms, ease-in). Reversing
 * mid-way continues from the current progress. The lobe parameters are seeded once per opening.
 */
@Stable
class LiquidRevealState internal constructor(initial: Float, seed: Long) {
    /** 0 = hidden, 1 = fully open. Read it in draw/layer lambdas to avoid recomposition. */
    val progress = Animatable(initial)

    private val phases = FloatArray(6)
    private val drops: Array<Droplet>
    private val path = Path()
    private val px = FloatArray(BLOT_POINTS)
    private val py = FloatArray(BLOT_POINTS)

    /** Window position of this reveal's root, so the origin can follow a sliding parent. */
    internal var rootInWindow by mutableStateOf(Offset.Zero)

    init {
        val random = Random(seed)
        for (i in phases.indices) phases[i] = random.nextFloat() * TAU
        drops = Array(4) { i ->
            Droplet(
                jitter = (random.nextFloat() - 0.5f) * 1.6f,
                distance = 0.22f + 0.16f * i + random.nextFloat() * 0.08f,
                size = 0.035f + random.nextFloat() * 0.03f,
                phase = random.nextFloat() * TAU,
            )
        }
    }

    /** Runs the reveal towards open or closed from wherever it currently is. */
    internal suspend fun run(open: Boolean, motion: Boolean) {
        val p = progress.value
        if (!motion) {
            progress.snapTo(if (open) 1f else 0f)
        } else if (open) {
            val span = ((1f - p) * Motion.PlayerOpenMs.toFloat()).toInt()
            if (span <= 0) progress.snapTo(1f) else progress.animateTo(1f, tween(span, easing = EaseOutQuart))
        } else {
            val span = (p * Motion.PlayerCloseMs.toFloat()).toInt()
            if (span <= 0) progress.snapTo(0f) else progress.animateTo(0f, tween(span, easing = EaseInCubic))
        }
    }

    /** Rebuilds the blot outline (main lobe plus leading droplets) for [share] of the way open. */
    internal fun outline(origin: Offset, width: Float, height: Float, share: Float): Path {
        path.rewind()
        val far = hypot(max(origin.x, width - origin.x), max(origin.y, height - origin.y))
        val radius = max(far * 1.12f * share, 1f)
        val wobble = 0.16f * (1f - share).coerceAtLeast(0f).pow(1.3f) + 0.025f
        // The ripple phase follows the growth, so the edge squirms while it spreads and settles.
        val seconds = share * 1.2f
        lobe(origin.x, origin.y, radius, wobble, seconds, 0, BLOT_POINTS)

        val toCentre = atan2(height / 2f - origin.y, width / 2f - origin.x)
        for (d in drops) {
            val dist = d.distance * far
            if (dist <= 0f) continue
            val lead = (radius / dist - 0.55f) / 0.45f
            if (lead <= 0f || lead >= 1.3f) continue
            val grow = sin(min(lead, 1f) * (TAU / 4f))
            val r = d.size * far * grow
            if (r < 1f) continue
            val angle = toCentre + d.jitter
            lobe(origin.x + cos(angle) * dist, origin.y + sin(angle) * dist, r, 0.12f, seconds + d.phase, 1, DROPLET_POINTS)
        }
        return path
    }

    /** A closed Catmull-Rom curve through [n] wobbling points around (cx, cy), as cubic Beziers. */
    private fun lobe(cx: Float, cy: Float, radius: Float, wobble: Float, seconds: Float, seed: Int, n: Int) {
        val p0 = phases[(seed * 3) % phases.size]
        val p1 = phases[(seed * 3 + 1) % phases.size]
        val p2 = phases[(seed * 3 + 2) % phases.size]
        for (i in 0 until n) {
            val a = i * TAU / n
            val ripple = 0.55f * sin(3f * a + seconds * 2.2f + p0) +
                0.30f * sin(5f * a - seconds * 1.5f + p1) +
                0.15f * sin(8f * a + seconds * 3.1f + p2)
            val r = radius * (1f + wobble * ripple)
            px[i] = cx + cos(a) * r
            py[i] = cy + sin(a) * r
        }
        path.moveTo(px[0], py[0])
        for (i in 0 until n) {
            val a = (i + n - 1) % n
            val c = (i + 1) % n
            val d = (i + 2) % n
            path.cubicTo(
                px[i] + (px[c] - px[a]) / 6f, py[i] + (py[c] - py[a]) / 6f,
                px[c] - (px[d] - px[i]) / 6f, py[c] - (py[d] - py[i]) / 6f,
                px[c], py[c],
            )
        }
        path.close()
    }
}

/**
 * Creates the reveal state and drives it: opening while [open] is true, closing when it turns false.
 * A screen that is re-created already open (rotation) starts fully revealed instead of replaying.
 */
@Composable
fun rememberLiquidReveal(open: Boolean): LiquidRevealState {
    var wasOpen by rememberSaveable { mutableStateOf(false) }
    val state = remember { LiquidRevealState(if (wasOpen) 1f else 0f, System.nanoTime()) }
    val motion = LocalMotionEnabled.current
    LaunchedEffect(open) {
        state.run(open, motion)
        wasOpen = open
    }
    return state
}

/**
 * Clips the content to the liquid blot growing from [origin] (window space; null = bottom centre)
 * and fills the blot with a glow-tinted void underneath. When fully open nothing extra is drawn.
 *
 * The glow is read from [LocalGlow] by the draw node itself, and only while the blot is moving, so
 * a cover-accent fade never recomposes the player that hosts the reveal.
 */
fun Modifier.liquidReveal(state: LiquidRevealState, origin: () -> Offset?): Modifier =
    this then LiquidRevealElement(state, origin)

private data class LiquidRevealElement(
    val state: LiquidRevealState,
    val origin: () -> Offset?,
) : ModifierNodeElement<LiquidRevealNode>() {
    override fun create() = LiquidRevealNode(state, origin)

    override fun update(node: LiquidRevealNode) {
        node.state = state
        node.origin = origin
    }

    override fun InspectorInfo.inspectableProperties() {
        name = "liquidReveal"
    }
}

private class LiquidRevealNode(
    var state: LiquidRevealState,
    var origin: () -> Offset?,
) : Modifier.Node(), DrawModifierNode, GlobalPositionAwareModifierNode, CompositionLocalConsumerModifierNode {
    override fun onGloballyPositioned(coordinates: LayoutCoordinates) {
        state.rootInWindow = coordinates.positionInWindow()
    }

    override fun ContentDrawScope.draw() {
        val p = state.progress.value
        when {
            p >= 1f -> drawContent()
            p <= 0.002f -> Unit
            else -> {
                val o = origin()?.minus(state.rootInWindow) ?: Offset(size.width / 2f, size.height - Dimens.SheetCover.toPx())
                val blot = state.outline(o, size.width, size.height, p)
                drawPath(blot, lerp(Orbit.Void, currentValueOf(LocalGlow), BLOT_GLOW))
                clipPath(blot) { this@draw.drawContent() }
            }
        }
    }
}

/** Fades the picture in once the blot covers most of the screen (the blot itself stays solid). */
fun Modifier.liquidContentAlpha(state: LiquidRevealState): Modifier =
    graphicsLayer { alpha = ((state.progress.value - 0.7f) / 0.3f).coerceIn(0f, 1f) }

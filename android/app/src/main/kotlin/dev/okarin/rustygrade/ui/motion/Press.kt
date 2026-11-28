package dev.okarin.rustygrade.ui.motion

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.AnimationVector1D
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.node.CompositionLocalConsumerModifierNode
import androidx.compose.ui.node.LayoutModifierNode
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.node.currentValueOf
import androidx.compose.ui.node.invalidatePlacement
import androidx.compose.ui.platform.InspectorInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Constraints
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.orbitTween
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * `clickable` plus the desktop press feedback (`HEX_PRESS` in, `HEX_HOVER` out): the target
 * shrinks to [pressedScale] while held. Touch has no hover, so the press is the only cue; it is
 * deliberately small so dense lists do not wobble.
 *
 * The scale lives in a [Modifier.Node] fed by the same interaction source as the click, so an idle
 * row costs no animation state and no recomposition: the layer only appears on the first press.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Modifier.pressable(
    enabled: Boolean = true,
    role: Role? = Role.Button,
    pressedScale: Float = 0.97f,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
): Modifier {
    val source = remember { MutableInteractionSource() }
    return this
        .pressScale(source, enabled, pressedScale)
        .combinedClickable(
            interactionSource = source,
            indication = LocalIndication.current,
            enabled = enabled,
            role = role,
            onLongClick = onLongClick,
            onClick = onClick,
        )
}

/**
 * The [pressable] shrink alone, for a target whose click handler lives elsewhere (a `toggleable`,
 * say), driven by that handler's [source].
 */
fun Modifier.pressScale(source: InteractionSource, enabled: Boolean = true, pressedScale: Float = 0.97f): Modifier =
    this then PressScaleElement(source, enabled, pressedScale)

private data class PressScaleElement(
    val source: InteractionSource,
    val enabled: Boolean,
    val pressedScale: Float,
) : ModifierNodeElement<PressScaleNode>() {
    override fun create() = PressScaleNode(source, enabled, pressedScale)

    override fun update(node: PressScaleNode) = node.update(source, enabled, pressedScale)

    override fun InspectorInfo.inspectableProperties() {
        name = "pressScale"
        properties["enabled"] = enabled
        properties["pressedScale"] = pressedScale
    }
}

private class PressScaleNode(
    private var source: InteractionSource,
    private var enabled: Boolean,
    private var pressedScale: Float,
) : Modifier.Node(), LayoutModifierNode, CompositionLocalConsumerModifierNode {
    /** Created on the first press; `null` (no layer) while the target has never been touched. */
    private var scale: Animatable<Float, AnimationVector1D>? = null
    private var collector: Job? = null

    override fun onAttach() = collect()

    // Detached nodes are re-measured on their next attach, so only an attached reset invalidates.
    override fun onDetach() = reset(invalidate = false)

    override fun onReset() = reset(invalidate = false)

    fun update(source: InteractionSource, enabled: Boolean, pressedScale: Float) {
        this.pressedScale = pressedScale
        if (this.enabled != enabled) {
            this.enabled = enabled
            if (!enabled) reset(invalidate = isAttached)
        }
        if (this.source != source) {
            this.source = source
            reset(invalidate = isAttached)
            if (isAttached) collect()
        }
    }

    private fun collect() {
        collector?.cancel()
        collector = coroutineScope.launch {
            val presses = mutableListOf<PressInteraction.Press>()
            source.interactions.collect { interaction ->
                when (interaction) {
                    is PressInteraction.Press -> presses.add(interaction)
                    is PressInteraction.Release -> presses.remove(interaction.press)
                    is PressInteraction.Cancel -> presses.remove(interaction.press)
                    else -> return@collect
                }
                animate(presses.isNotEmpty())
            }
        }
    }

    private fun animate(down: Boolean) {
        if (!enabled) return
        val anim = scale ?: Animatable(1f).also {
            scale = it
            invalidatePlacement()
        }
        val motion = currentValueOf(LocalMotionEnabled)
        val target = if (down) pressedScale else 1f
        coroutineScope.launch {
            anim.animateTo(target, orbitTween(if (down) Motion.PressMs else Motion.HoverMs, motion))
        }
    }

    /** Back to rest: no scale, no layer. Used on detach, reuse in a lazy list, and disable. */
    private fun reset(invalidate: Boolean) {
        if (scale != null) {
            scale = null
            if (invalidate) invalidatePlacement()
        }
    }

    override fun MeasureScope.measure(measurable: Measurable, constraints: Constraints): MeasureResult {
        val placeable = measurable.measure(constraints)
        return layout(placeable.width, placeable.height) {
            val anim = scale
            if (anim == null) {
                placeable.place(0, 0)
            } else {
                placeable.placeWithLayer(0, 0) {
                    val s = anim.value
                    scaleX = s
                    scaleY = s
                }
            }
        }
    }
}

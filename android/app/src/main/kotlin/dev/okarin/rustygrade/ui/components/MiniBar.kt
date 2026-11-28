package dev.okarin.rustygrade.ui.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.basicMarquee
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.LocalPowerSaveMode
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.motion.pressable
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.orbitTween
import kotlin.math.abs

/**
 * Persistent mini player above the bottom navigation: cover, title/artist, quick skip next,
 * hex play/pause and a thin progress line along the top edge (the desktop transport line: the
 * Lonetrail spectrum over a soft rule). Tap or swipe up expands; swipe left/right skips tracks.
 */
@Composable
fun MiniBar(onExpand: () -> Unit, modifier: Modifier = Modifier) {
    val env = LocalEnv.current
    val state by PlaybackHub.state.collectAsStateWithLifecycle()
    // The real state for the play/pause button; the shell's LocalIsPlaying (false while
    // backgrounded or covered by the player) only gates the decorative marquee.
    val isPlaying by PlaybackHub.isPlaying.collectAsStateWithLifecycle()
    val powerSave = LocalPowerSaveMode.current
    val motion = LocalMotionEnabled.current
    val marqueeActive = LocalIsPlaying.current && motion && !powerSave
    // Position/duration are read inside the draw lambda only: a tick redraws the 2 dp line, it does
    // not recompose the bar. Collection stops with the lifecycle, and the read is skipped while a
    // full-screen overlay covers the bar.
    val positionMs = PlaybackHub.positionMs.collectAsStateWithLifecycle()
    val durationMs = PlaybackHub.durationMs.collectAsStateWithLifecycle()
    val density = LocalDensity.current
    val threshold = with(density) { 64.dp.toPx() }
    val upThreshold = with(density) { 48.dp.toPx() }
    val maxNudgePx = with(density) { 28.dp.toPx() }
    val expand = rememberUpdatedState(onExpand)
    // Window position of the hex button's centre: the full-screen player's blot grows from here.
    val hexCenter = remember { mutableStateOf<Offset?>(null) }
    var dragNudgeX by remember { mutableFloatStateOf(0f) }
    val animatedNudgeX by animateFloatAsState(
        targetValue = if (powerSave || !motion) 0f else dragNudgeX,
        animationSpec = orbitTween(Motion.PressMs, motion),
        label = "minibar-nudge",
    )
    val track = state?.current ?: return
    val canNext = state?.canNext == true

    Box(
        modifier
            .fillMaxWidth()
            .background(Orbit.Panel)
            .drawBehind {
                val rule = Size(size.width, Dimens.Rule.toPx())
                drawRect(Orbit.LineSoft, Offset.Zero, rule)
                if (env.nav.playerOpen) return@drawBehind
                val d = durationMs.value
                val f = if (d > 0) (positionMs.value.toFloat() / d).coerceIn(0f, 1f) else 0f
                // The full-width spectrum, revealed up to the elapsed fraction.
                if (f > 0f) clipRect(right = size.width * f) { drawRect(SpectrumBrush, Offset.Zero, rule) }
            }
            .semantics {
                // Swipe gestures have no TalkBack equivalent: expose them as actions.
                customActions = listOf(
                    CustomAccessibilityAction("Expand player") {
                        env.nav.playerOrigin = hexCenter.value
                        expand.value()
                        true
                    },
                    CustomAccessibilityAction("Next track") { env.playback.next(); true },
                    CustomAccessibilityAction("Previous track") { env.playback.previous(); true },
                )
            }
            .pointerInput(threshold, upThreshold, maxNudgePx) {
                var totalX = 0f
                var totalY = 0f
                detectDragGestures(
                    onDragStart = {
                        totalX = 0f
                        totalY = 0f
                        dragNudgeX = 0f
                    },
                    onDragCancel = {
                        totalX = 0f
                        totalY = 0f
                        dragNudgeX = 0f
                    },
                    onDragEnd = {
                        when {
                            abs(totalX) >= abs(totalY) && totalX <= -threshold -> env.playback.next()
                            abs(totalX) >= abs(totalY) && totalX >= threshold -> env.playback.previous()
                            totalY < -upThreshold && abs(totalY) > abs(totalX) -> {
                                env.nav.playerOrigin = hexCenter.value
                                expand.value()
                            }
                        }
                        totalX = 0f
                        totalY = 0f
                        dragNudgeX = 0f
                    },
                    onDrag = { change, amount ->
                        change.consume()
                        totalX += amount.x
                        totalY += amount.y
                        dragNudgeX = if (abs(totalX) >= abs(totalY)) {
                            (totalX * 0.25f).coerceIn(-maxNudgePx, maxNudgePx)
                        } else {
                            0f
                        }
                    },
                )
            }
            .pressable(pressedScale = 0.99f) {
                env.nav.playerOrigin = hexCenter.value
                expand.value()
            },
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .graphicsLayer { translationX = animatedNudgeX }
                .padding(start = Space.Gutter, end = Dimens.RowPadV, top = Dimens.RowPadV, bottom = Dimens.RowPadV),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Dimens.RowPadV),
        ) {
            Cover(track.thumbPath, size = Dimens.RowCover)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.Hair)) {
                // The marquee animation only runs while playing, visible and not in battery saver.
                Text(
                    track.title,
                    modifier = Modifier.fillMaxWidth().marqueeWhile(marqueeActive),
                    color = Orbit.Text,
                    style = OrbitType.RowTitle,
                    maxLines = 1,
                    softWrap = false,
                    overflow = if (marqueeActive) TextOverflow.Clip else TextOverflow.Ellipsis,
                )
                Text(
                    track.artist,
                    modifier = Modifier.fillMaxWidth().marqueeWhile(marqueeActive),
                    color = Orbit.TextDim,
                    style = OrbitType.RowMeta,
                    maxLines = 1,
                    softWrap = false,
                    overflow = if (marqueeActive) TextOverflow.Clip else TextOverflow.Ellipsis,
                )
            }
            Box(
                Modifier.size(Dimens.Touch).clickable(enabled = canNext, role = Role.Button) { env.playback.next() },
                contentAlignment = Alignment.Center,
            ) {
                Icon(OrbitIcons.Skipforward, "Next track", tint = if (canNext) Orbit.Text else Orbit.TextMute, modifier = Modifier.size(Dimens.IconMd))
            }
            Box(
                Modifier.height(48.dp).onGloballyPositioned { coords ->
                    hexCenter.value = coords.positionInWindow() + Offset(coords.size.width / 2f, coords.size.height / 2f)
                },
                contentAlignment = Alignment.Center,
            ) {
                HexButton(playing = isPlaying, onClick = { env.playback.togglePlay() }, size = Dimens.Touch, accent = LocalAccent.current)
            }
        }
    }
}

private fun Modifier.marqueeWhile(enabled: Boolean): Modifier =
    if (enabled) basicMarquee(iterations = Int.MAX_VALUE) else this

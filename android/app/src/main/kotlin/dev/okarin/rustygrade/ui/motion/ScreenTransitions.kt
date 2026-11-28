package dev.okarin.rustygrade.ui.motion

import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.ui.unit.IntOffset
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.orbitTween

/** Where a screen change goes, which decides the direction of its transition. */
enum class ScreenMove { Push, Pop, Switch }

/**
 * Push slides the new screen in from the right over a short fade (the old one drifts left and
 * fades); pop is the mirror; switching tabs fades with the page rise. All cubic-out, no bounce:
 * the HUD moves like an instrument panel, not a sheet of paper.
 */
fun <S> AnimatedContentTransitionScope<S>.orbitScreenTransform(move: ScreenMove, enabled: Boolean): ContentTransform {
    if (!enabled) return (EnterTransition.None togetherWith ExitTransition.None).using(SizeTransform(clip = false))
    val slide = orbitTween<IntOffset>(Motion.ScreenMs, true)
    val fadeInSpec = orbitTween<Float>(Motion.ScreenMs, true)
    val fadeOutSpec = orbitTween<Float>(Motion.ScreenMs / 2, true)
    val transform = when (move) {
        ScreenMove.Push ->
            (slideInHorizontally(slide) { it / 5 } + fadeIn(fadeInSpec)) togetherWith
                (slideOutHorizontally(slide) { -it / 12 } + fadeOut(fadeOutSpec))
        ScreenMove.Pop ->
            (slideInHorizontally(slide) { -it / 12 } + fadeIn(fadeInSpec)) togetherWith
                (slideOutHorizontally(slide) { it / 5 } + fadeOut(fadeOutSpec))
        ScreenMove.Switch ->
            (fadeIn(orbitTween(Motion.PageEnterMs, true)) + slideInVertically(orbitTween<IntOffset>(Motion.PageEnterMs, true)) { it / 60 }) togetherWith
                fadeOut(orbitTween(Motion.PageEnterMs / 2, true))
    }
    return transform.using(SizeTransform(clip = false))
}

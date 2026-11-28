package dev.okarin.rustygrade.ui.theme

import android.content.ContentResolver
import android.database.ContentObserver
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp

/**
 * Motion tokens, named after the desktop `motion` module (`Ease`, `EnterSpec`, `HEX_HOVER`, ...)
 * so both ports animate alike.
 */
object Motion {
    /** `motion::Ease`. */
    object Ease {
        val CubicOut: Easing = CubicBezierEasing(0.215f, 0.61f, 0.355f, 1f)
        val CubicInOut: Easing = CubicBezierEasing(0.645f, 0.045f, 0.355f, 1f)
    }

    /** `AccentAnimator::FADE_DURATION`: cover accent cross-fade. */
    const val AccentFadeMs = 400

    /** `HEX_HOVER` / `HEX_PRESS`: press feedback in and out. */
    const val HoverMs = 180
    const val PressMs = 90

    /** Page entrance (`EnterSpec::new(300, CubicOut, 8.0, ..)` on library / playlists). */
    const val PageEnterMs = 300

    /** Detail-screen hero (cover, backdrop) entrance: a page entrance plus a tab fade. */
    const val HeroRevealMs = PageEnterMs + 120
    val PageEnterRise = 8.dp

    /** Window after a scope opens in which an item's first sight still plays its entrance. */
    const val EnterWindowMs = 700L

    /** Push / pop between screens. */
    const val ScreenMs = 280

    /** Grid / list item cascade: per-item delay and the index after which items arrive together. */
    const val StaggerMs = 28
    const val StaggerCap = 10

    /** `enter::ABSENCE`: a view gone for less than this does not replay its entrance. */
    const val AbsenceMs = 100

    /** Playing-dot pulse period (`pulse_dot`). */
    const val PulseMs = 1100

    /** Cross-fade between the player tabs (art / lyrics / queue). */
    const val TabFadeMs = 120

    /** Liquid reveal of the full player: open, close, and when the closing layer is removed. */
    const val PlayerOpenMs = 700
    const val PlayerCloseMs = 420
    const val PlayerExitDelayMs = 425

    /** Artwork backdrop cross-fade between covers. */
    const val BackdropFadeMs = 420
}

/**
 * Whether decorative motion runs. Off in Battery Saver and when the system animator duration
 * scale is 0 ("Remove animations"), like the desktop `motion::set_enabled`. Transitions then jump
 * straight to their end state.
 */
val LocalMotionEnabled = compositionLocalOf { true }

/**
 * Whether the system "animator duration scale" is above 0. Observed live, so toggling "Remove
 * animations" applies without restarting the activity.
 */
@Composable
fun rememberSystemAnimationsOn(): Boolean {
    val resolver = LocalContext.current.contentResolver
    var on by remember(resolver) { mutableStateOf(readAnimationsOn(resolver)) }
    DisposableEffect(resolver) {
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                on = readAnimationsOn(resolver)
            }
        }
        resolver.registerContentObserver(
            Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE),
            false,
            observer,
        )
        on = readAnimationsOn(resolver)
        onDispose { resolver.unregisterContentObserver(observer) }
    }
    return on
}

private fun readAnimationsOn(resolver: ContentResolver): Boolean =
    Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) > 0f

/** A cubic-out tween of [ms], or a snap when motion is off. */
fun <T> orbitTween(ms: Int, enabled: Boolean, delayMs: Int = 0, easing: Easing = Motion.Ease.CubicOut): FiniteAnimationSpec<T> =
    if (enabled) tween(ms, delayMs, easing) else snap()

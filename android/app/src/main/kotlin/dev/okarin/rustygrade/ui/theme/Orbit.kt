package dev.okarin.rustygrade.ui.theme

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.State
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import kotlin.math.pow

/** Orbit Terminal palette, adapted from `reference/theme.rs` for mobile contrast. */
object Orbit {
    val Void = Color(0xFF08080A)
    val Panel = Color(0xFF121215)
    val PanelDeep = Color(0xFF0D0D10)
    val Card = Color(0xFF17171B)
    val Raised = Color(0xFF1D1D22)
    val Hover = Color(0xFF28282F)
    val Line = Color(0xFF34343C)
    val LineSoft = Color(0xFF25252B)
    val Teal = Color(0xFF4FD6BE)

    /** Terminal "online" LED green. */
    val Green = Color(0xFF4ADE6B)
    val Orange = Color(0xFFFF6B35)
    val Gold = Color(0xFFE8C060)
    val Text = Color(0xFFF2EFE8)
    val TextDim = Color(0xFFB5B2AA)
    val TextMute = Color(0xFF87847D)
    val Danger = Color(0xFFEF5B5B)

    /** Orbit's default accent / glow when there is no (colorful) cover. */
    val DefaultAccent = Color(0xFFFF6B35)
    val DefaultGlow = Color(0xFFD95626)
}

/** The current accent (follows the playing cover when "Dynamic Cover Accent" is on). */
val LocalAccent = compositionLocalOf { Orbit.DefaultAccent }

/** The ambient glow color that goes with the accent. */
val LocalGlow = compositionLocalOf { Orbit.DefaultGlow }

val Mono = FontFamily.Monospace
val Sans = FontFamily.SansSerif

private fun mono(size: Int, weight: FontWeight = FontWeight.Normal, tracking: Double = 0.0, lineHeight: Int = size + 4) =
    TextStyle(fontFamily = Mono, fontSize = size.sp, fontWeight = weight, letterSpacing = tracking.sp, lineHeight = lineHeight.sp)

private fun sans(size: Int, weight: FontWeight = FontWeight.Normal, lineHeight: Int = size + 6) =
    TextStyle(fontFamily = Sans, fontSize = size.sp, fontWeight = weight, lineHeight = lineHeight.sp)

private val OrbitTypography = Typography(
    displayLarge = mono(32, FontWeight.Bold, lineHeight = 38),
    displayMedium = mono(28, FontWeight.Bold, lineHeight = 34),
    displaySmall = mono(24, FontWeight.Bold, lineHeight = 30),
    headlineLarge = sans(24, FontWeight.Bold, lineHeight = 30),
    headlineMedium = sans(21, FontWeight.Bold, lineHeight = 27),
    headlineSmall = sans(18, FontWeight.Bold, lineHeight = 24),
    titleLarge = sans(17, FontWeight.Bold, lineHeight = 23),
    titleMedium = sans(15, FontWeight.SemiBold, lineHeight = 20),
    titleSmall = sans(13, FontWeight.SemiBold, lineHeight = 18),
    bodyLarge = sans(15, FontWeight.Normal, lineHeight = 22),
    bodyMedium = sans(14, FontWeight.Normal, lineHeight = 20),
    bodySmall = sans(12, FontWeight.Normal, lineHeight = 17),
    labelLarge = mono(12, FontWeight.Bold, 1.0, lineHeight = 16),
    labelMedium = mono(11, FontWeight.Bold, 0.8, lineHeight = 15),
    labelSmall = mono(10, FontWeight.Bold, 0.8, lineHeight = 14),
)

/** Hard edges everywhere, like the desktop UI. */
private val SquareShapes = Shapes(
    extraSmall = RoundedCornerShape(0),
    small = RoundedCornerShape(0),
    medium = RoundedCornerShape(0),
    large = RoundedCornerShape(0),
    extraLarge = RoundedCornerShape(0),
)

@Composable
fun OrbitTheme(
    accent: Color = Orbit.DefaultAccent,
    glow: Color = Orbit.DefaultGlow,
    content: @Composable () -> Unit,
) {
    val scheme = remember(accent) {
        darkColorScheme(
            primary = accent,
            onPrimary = readableTextOn(accent),
            secondary = Orbit.Teal,
            onSecondary = Orbit.Void,
            tertiary = Orbit.Gold,
            background = Orbit.Void,
            onBackground = Orbit.Text,
            surface = Orbit.Panel,
            onSurface = Orbit.Text,
            surfaceVariant = Orbit.Card,
            onSurfaceVariant = Orbit.TextDim,
            surfaceContainer = Orbit.Raised,
            surfaceContainerHigh = Orbit.Raised,
            surfaceContainerHighest = Orbit.Hover,
            outline = Orbit.Line,
            outlineVariant = Orbit.LineSoft,
            error = Orbit.Danger,
        )
    }
    CompositionLocalProvider(LocalAccent provides accent, LocalGlow provides glow) {
        MaterialTheme(colorScheme = scheme, typography = OrbitTypography, shapes = SquareShapes, content = content)
    }
}

/** 400 ms ease-out-cubic, the fade the desktop `AccentAnimator` uses between covers. */
@Composable
fun animatedAccent(target: Color): State<Color> =
    animateColorAsState(target, tween(Motion.AccentFadeMs, easing = Motion.Ease.CubicOut), label = "accent")

fun Color.withAlpha(alpha: Float): Color = copy(alpha = alpha)

/** `0xRRGGBB` from the Rust palette to an opaque [Color]. */
fun colorOfRgb(rgb: Long): Color = Color(0xFF000000 or (rgb and 0xFFFFFF))

/** WCAG relative luminance of an sRGB color. */
fun relativeLuminance(c: Color): Float {
    fun lin(v: Float) = if (v <= 0.04045f) v / 12.92f else ((v + 0.055f) / 1.055f).pow(2.4f)
    return 0.2126f * lin(c.red) + 0.7152f * lin(c.green) + 0.0722f * lin(c.blue)
}

/** WCAG contrast ratio of two colors. */
fun contrastRatio(a: Color, b: Color): Float {
    val la = relativeLuminance(a)
    val lb = relativeLuminance(b)
    return (maxOf(la, lb) + 0.05f) / (minOf(la, lb) + 0.05f)
}

/** The text color (void or light) that reads better on [bg]. */
fun readableTextOn(bg: Color): Color =
    if (contrastRatio(bg, Orbit.Void) >= contrastRatio(bg, Orbit.Text)) Orbit.Void else Orbit.Text

/** Keeps [accent] when it meets [minContrast] on [bg]; otherwise lifts it toward [Orbit.Text]. */
fun readableAccentOn(bg: Color, accent: Color, minContrast: Float = 4.5f): Color {
    if (contrastRatio(bg, accent) >= minContrast) return accent
    for (step in 1..10) {
        val candidate = lerp(accent, Orbit.Text, step / 10f)
        if (contrastRatio(bg, candidate) >= minContrast) return candidate
    }
    return Orbit.Text
}

/** [readableAccentOn] memoized per (bg, accent): the contrast search runs once per accent change. */
@Composable
fun rememberReadableAccentOn(bg: Color, accent: Color = LocalAccent.current): Color =
    remember(bg, accent) { readableAccentOn(bg, accent) }

/**
 * Hair spaces between the characters of a short static header (the desktop `tracked()`): wide HUD
 * labels. Never apply it to user metadata.
 */
fun tracked(s: String): String = s.toList().joinToString(" ")

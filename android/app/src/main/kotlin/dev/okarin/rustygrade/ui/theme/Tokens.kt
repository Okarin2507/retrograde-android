package dev.okarin.rustygrade.ui.theme

import androidx.compose.ui.unit.dp

/**
 * Spacing scale. Every gap and padding in the app is one of these steps; one-off values (art
 * sizes, a single hand-tuned offset) stay local, as they do in the desktop build.
 */
object Space {
    val Hair = 2.dp
    val Xs = 4.dp
    val Compact = 6.dp
    val S = 8.dp
    val M = 12.dp
    val L = 16.dp
    val Xl = 20.dp
    val Xxl = 24.dp
    val Xxxl = 32.dp

    /** Side gutter of every screen. */
    val Gutter = L
}

/** Recurring component sizes. */
object Dimens {
    /** Minimum touch target (Material / WCAG 2.5.8). */
    val Touch = 44.dp
    val Stroke = 1.dp

    /** The accent bar before a screen title (the desktop `LIBRARY_DATA` bar). */
    val TitleBarWidth = 3.dp
    val TitleBarHeight = 18.dp

    /** Active-tab and spectrum rules. */
    val Rule = 2.dp

    val IconXs = 15.dp
    val IconS = 16.dp
    val IconMd = 18.dp
    val IconM = 20.dp
    val IconL = 22.dp

    /** The square `10/01` bit-perfect glyph. */
    val BitGlyph = 26.dp

    /** Vertical padding of a list row and the gap between its leading art and its text. */
    val RowPadV = 10.dp
    val RowGap = 14.dp

    val HeaderHeight = 56.dp
    val NavBarHeight = 60.dp

    /** Corner tick (L-bracket) arm on covers and cards. */
    val CornerTick = 14.dp

    /** Small corner tick on dense tiles, and the dash of a kicker label. */
    val TickS = 10.dp
    val RowCover = 44.dp

    /** Cover thumbnail in a bottom sheet header. */
    val SheetCover = 48.dp

    /** Height of a single-line text field. */
    val FieldHeight = 46.dp

    /** Blur radius of the artwork backdrop behind the player. */
    val BackdropBlur = 48.dp
}

/**
 * Alpha tiers, so translucent layers stay consistent: a [Wash] under a tinted control, a [Tint]
 * on an active one, a [Scrim] over artwork, and the text tiers used on top of artwork.
 */
object Alpha {
    const val Wash = 0.08f
    const val Tint = 0.16f
    const val Faint = 0.28f
    const val HeroFade = 0.30f
    const val ArtGlow = 0.35f
    const val HeroScrim = 0.42f
    const val BorderSoft = 0.45f
    const val Muted = 0.55f
    const val Border = 0.60f
    const val Scrim = 0.62f
    const val LyricUpcoming = 0.75f
    const val BackdropTop = 0.78f
    const val ScrimMid = 0.82f
    const val TextOnArt = 0.84f
    const val LyricPlain = 0.90f
    const val ScrimDeep = 0.95f
}

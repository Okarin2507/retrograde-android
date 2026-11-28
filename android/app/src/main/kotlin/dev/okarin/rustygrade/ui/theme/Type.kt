package dev.okarin.rustygrade.ui.theme

import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/**
 * Named text roles of the Orbit HUD. Monospace tracked caps for chrome (titles, chips, buttons,
 * labels), sans for user metadata (titles of albums and tracks, artists). Color is left to the
 * caller: the same role is drawn dim, bright or in the accent depending on state.
 */
object OrbitType {
    private fun mono(size: Double, weight: FontWeight, tracking: Double, line: Double = size + 4) =
        TextStyle(fontFamily = Mono, fontSize = size.sp, fontWeight = weight, letterSpacing = tracking.sp, lineHeight = line.sp)

    private fun sans(size: Double, weight: FontWeight, line: Double = size + 6) =
        TextStyle(fontFamily = Sans, fontSize = size.sp, fontWeight = weight, lineHeight = line.sp)

    /** Screen title next to the accent bar: `LIBRARY`, `PLAYLISTS`. */
    val ScreenTitle = mono(15.0, FontWeight.Bold, 2.0)

    /** Big hero title on detail screens (album / playlist name stays sans: it is user metadata). */
    val HeroTitle = sans(22.0, FontWeight.Bold, 28.0)

    /** Section header, tracked: `DISC 1`, `SMART PLAYLISTS`. */
    val Section = mono(11.0, FontWeight.Bold, 1.6)

    /** The teal `— SYSTEM_NAV` kicker of the desktop sidebar. */
    val Kicker = mono(10.0, FontWeight.Bold, 1.2)

    val Button = mono(12.0, FontWeight.Bold, 0.8, 16.0)
    val Chip = mono(11.0, FontWeight.Bold, 1.0)
    val NavLabel = mono(10.0, FontWeight.Medium, 0.8)
    val Badge = mono(10.0, FontWeight.SemiBold, 0.5)

    /** The tracked `KIND // meta` line under a detail hero title. */
    val HeroMeta = mono(10.0, FontWeight.SemiBold, 0.8)

    /** `year · N TRACKS` under an album card. */
    val CardBadge = mono(10.0, FontWeight.SemiBold, 0.4)
    val Numeric = mono(12.0, FontWeight.Normal, 0.0)
    val Hud = mono(12.0, FontWeight.Normal, 1.0, 18.0)
    val HudBold = mono(12.0, FontWeight.Bold, 1.0, 18.0)
    val NumericBold = mono(12.0, FontWeight.Bold, 0.0)
    val NavLabelActive = mono(10.0, FontWeight.Bold, 0.8)

    /** The player's `ART` / `LYRICS` / `QUEUE` switcher. */
    val TabChip = mono(11.0, FontWeight.Medium, 0.9)
    val TabChipActive = mono(11.0, FontWeight.Bold, 0.9)
    val MicroBadge = mono(9.0, FontWeight.Bold, 0.8)

    /** The two-line `10/01` bit-perfect glyph inside its square frame. */
    val BitGlyph = mono(8.5, FontWeight.Bold, 0.0, 9.0)

    /** Small mono status lines: scan progress, counts, banners. */
    val Status = mono(11.0, FontWeight.Normal, 0.0)
    val StatusBold = mono(11.0, FontWeight.Bold, 0.0)
    val Counter = mono(11.0, FontWeight.Normal, 0.6)
    val CountBold = mono(12.0, FontWeight.Bold, 0.6)

    /** The library's `ALBUMS` / `ARTISTS` view switch. */
    val ViewTab = mono(12.0, FontWeight.Medium, 1.0)
    val ViewTabActive = mono(12.0, FontWeight.Bold, 1.0)

    /** Dialog / sheet heading and the terminal-style panel title. */
    val DialogTitle = mono(14.0, FontWeight.Bold, 1.6)
    val TerminalTitle = mono(13.0, FontWeight.Bold, 0.6)

    val RowTitle = sans(14.0, FontWeight.SemiBold, 19.0)
    val RowTitleActive = sans(14.0, FontWeight.Bold, 19.0)

    /** Lighter row title for dense secondary lists (the queue). */
    val RowTitleSoft = sans(14.0, FontWeight.Medium, 19.0)
    val RowMeta = sans(12.0, FontWeight.Normal, 16.0)
    val CardTitle = sans(13.0, FontWeight.SemiBold, 17.0)

    /** Title in a bottom-sheet header (album / playlist name next to its cover). */
    val SheetTitle = sans(15.0, FontWeight.SemiBold, 20.0)
    val CardMeta = sans(12.0, FontWeight.Normal, 16.0)

    /** Title under a small picker tile (online cover candidates). */
    val TileTitle = sans(12.0, FontWeight.SemiBold, 16.0)

    /** Artist list row: the name and its `N ALBUMS · M TRACKS` line. */
    val ArtistName = sans(15.0, FontWeight.Medium, 20.0)
    val ArtistMeta = mono(11.0, FontWeight.Normal, 0.4)
    val Body = sans(13.0, FontWeight.Normal, 18.0)

    /** Compact two-line preview rows (smart playlist matches). */
    val PreviewTitle = sans(13.0, FontWeight.Medium, 18.0)
    val PreviewMeta = sans(11.0, FontWeight.Normal, 15.0)
    val Input = sans(14.0, FontWeight.Normal, 20.0)

    /** Mono body for multi-line editors (synced lyrics). */
    val Code = mono(12.0, FontWeight.Normal, 0.0, 18.0)

    /** Text inside a `BasicTextField`, which takes its color from the style. */
    val FieldText = Input.copy(color = Orbit.Text)
    val FieldCode = Code.copy(color = Orbit.Text)

    /** Typed search text: sans, a step above [Body] for legibility. */
    val FieldSearch = sans(15.0, FontWeight.Normal, 20.0).copy(color = Orbit.Text)

    /** Title / artist under the full-player artwork. */
    val PlayerTitle = sans(16.0, FontWeight.Bold, 21.0)
    val PlayerArtist = sans(15.0, FontWeight.Normal, 21.0)

    /** Lyrics: an unsynced line, the active synced line, and its neighbours. */
    val LyricLine = sans(16.0, FontWeight.Normal, 24.0)
    val LyricActive = sans(20.0, FontWeight.Bold, 28.0)
    val LyricMid = sans(16.0, FontWeight.Medium, 24.0)
}

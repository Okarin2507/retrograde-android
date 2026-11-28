package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha

/** The Lonetrail spectrum: orange → gold → teal, the desktop top rule and transport line. */
val SpectrumBrush: Brush = Brush.horizontalGradient(
    0.00f to Orbit.Orange,
    0.50f to Orbit.Gold,
    1.00f to Orbit.Teal,
)

/** The spectrum as a hairline rule across the top of the shell. */
@Composable
fun SpectrumBar(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().height(Dimens.Rule).background(SpectrumBrush))
}

/** Desktop-style top-left and bottom-right L-bracket frame overlay (`corner_tick`). */
@Composable
fun CornerBrackets(
    modifier: Modifier = Modifier,
    length: Dp = Dimens.CornerTick,
    stroke: Dp = Dimens.Rule,
    topLeft: Color = Orbit.Orange,
    bottomRight: Color? = Orbit.Teal,
) {
    Canvas(modifier) {
        val len = length.toPx().coerceAtMost(minOf(size.width, size.height) * 0.45f)
        val thick = stroke.toPx()
        if (topLeft.alpha > 0f) {
            drawRect(topLeft, Offset.Zero, Size(len, thick))
            drawRect(topLeft, Offset.Zero, Size(thick, len))
        }
        if (bottomRight != null) {
            val rx = (size.width - len).coerceAtLeast(0f)
            val ry = (size.height - len).coerceAtLeast(0f)
            drawRect(bottomRight, Offset(rx, (size.height - thick).coerceAtLeast(0f)), Size(len, thick))
            drawRect(bottomRight, Offset((size.width - thick).coerceAtLeast(0f), ry), Size(thick, len))
        }
    }
}

/**
 * Square `10/01` glyph shown in place of a `BIT-PERFECT` label while the Direct USB driver streams
 * the untouched PCM to the DAC.
 */
@Composable
fun BitPerfectGlyph(modifier: Modifier = Modifier, color: Color = Orbit.Teal) {
    // A 2x2 grid of bits, each digit centred in its own quadrant so they read as a chip, not text.
    Column(
        modifier
            .size(Dimens.BitGlyph)
            .background(color.withAlpha(Alpha.Wash))
            .border(Dimens.Stroke, color)
            .padding(Space.Hair)
            .semantics { contentDescription = "Bit-perfect output" },
    ) {
        for (bits in BIT_GLYPH_ROWS) {
            Row(Modifier.weight(1f).fillMaxWidth()) {
                for (bit in bits) {
                    Box(Modifier.weight(1f).fillMaxSize(), contentAlignment = Alignment.Center) {
                        Text(bit.toString(), color = color, style = OrbitType.BitGlyph, textAlign = TextAlign.Center, maxLines = 1)
                    }
                }
            }
        }
    }
}

private val BIT_GLYPH_ROWS = listOf("10", "01")

/** Clean borderless audio quality label, e.g. `FLAC / 24BIT / 96KHZ`. */
@Composable
fun QualityPill(text: String, modifier: Modifier = Modifier, accent: Boolean = false) {
    val fg = if (accent) rememberReadableAccentOn(Orbit.Void, LocalAccent.current) else Orbit.TextDim
    Text(
        text,
        modifier = modifier,
        color = fg,
        style = OrbitType.Badge,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}

/** Crisp tracked section header such as `DISC 1`. */
@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier, color: Color = Orbit.TextDim) {
    Text(text.uppercase(), modifier = modifier, color = color, style = OrbitType.Section)
}

/**
 * The desktop sidebar kicker, `— SYSTEM_NAV`: a short teal dash and a tracked teal label, used
 * above grouped settings and section blocks.
 */
@Composable
fun Kicker(text: String, modifier: Modifier = Modifier, color: Color = Orbit.Teal) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.S)) {
        Box(Modifier.width(Dimens.TickS).height(Dimens.Stroke).background(color))
        Text(text.uppercase(), color = color, style = OrbitType.Kicker, maxLines = 1)
    }
}

@Composable
fun Divider(modifier: Modifier = Modifier) {
    Spacer(modifier.fillMaxWidth().size(Dimens.Stroke).background(Orbit.LineSoft))
}

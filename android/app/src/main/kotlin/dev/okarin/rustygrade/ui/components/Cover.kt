package dev.okarin.rustygrade.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import coil3.compose.AsyncImage
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.Orbit
import java.io.File

/** Cover art from a cache file, or a disc glyph when there is none. */
@Composable
fun Cover(path: String?, modifier: Modifier = Modifier, size: Dp? = null, bordered: Boolean = true) {
    val file = remember(path) { path?.let(::File) }
    val sized = if (size != null) modifier.size(size) else modifier
    val base = sized
        .background(Orbit.Card)
        .then(if (bordered) Modifier.border(BorderStroke(Dimens.Stroke, Orbit.LineSoft)) else Modifier)
    if (file == null) {
        Box(base, contentAlignment = Alignment.Center) {
            Icon(OrbitIcons.Disc, null, tint = Orbit.TextMute, modifier = Modifier.fillMaxSize(0.4f))
        }
    } else {
        AsyncImage(file, null, base, contentScale = ContentScale.Crop)
    }
}

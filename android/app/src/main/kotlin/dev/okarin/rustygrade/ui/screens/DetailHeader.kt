package dev.okarin.rustygrade.ui.screens

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.okarin.rustygrade.ui.components.CornerBrackets
import dev.okarin.rustygrade.ui.components.Cover
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.QualityPill
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.LocalGlow
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Motion
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.orbitTween
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import dev.okarin.rustygrade.ui.theme.withAlpha
import java.io.File

/** Result of an async detail load: [value] is null when the item does not exist. */
internal class DetailLoad<T>(val value: T?)

/** PLAY and SHUFFLE buttons shared by the album, artist and playlist headers. */
@Composable
internal fun DetailActionButtons(
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    playLabel: String = "PLAY",
) {
    Row(
        modifier.fillMaxWidth().padding(horizontal = Space.Gutter),
        horizontalArrangement = Arrangement.spacedBy(Dimens.RowPadV),
    ) {
        OrbitButton(
            playLabel, onPlay,
            modifier = Modifier.weight(1f).heightIn(min = Dimens.Touch),
            primary = true, icon = OrbitIcons.Play, enabled = enabled,
        )
        OrbitButton(
            "SHUFFLE", onShuffle,
            modifier = Modifier.weight(1f).heightIn(min = Dimens.Touch),
            icon = OrbitIcons.Shuffle, enabled = enabled,
        )
    }
}

/**
 * Hero block of a detail screen (album, artist, playlist): the cover blown up and blurred behind
 * everything (API 31+; older phones get it dimmed only), tinted by the cover glow and faded into
 * the void; a large framed cover with the desktop corner ticks; the title (user metadata, sans),
 * an optional accent [subtitle] link, a `KIND // meta` kicker and the quality badge; then
 * [actions] (PLAY / SHUFFLE). [header] is the screen's back / menu row, drawn over the backdrop.
 */
@Composable
internal fun DetailHero(
    coverPath: String?,
    title: String,
    meta: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    onSubtitle: (() -> Unit)? = null,
    quality: String? = null,
    coverOverlay: @Composable BoxScope.() -> Unit = {},
    header: @Composable () -> Unit,
    actions: @Composable () -> Unit,
) {
    val glow = LocalGlow.current
    val coverFile = remember(coverPath) { coverPath?.let(::File) }
    // Glow tint at the top, fading into the void so the track list reads on a flat ground.
    val tint = remember(glow) {
        Brush.verticalGradient(
            0f to glow.withAlpha(Alpha.HeroFade),
            0.45f to Orbit.Void.withAlpha(Alpha.Muted),
            1f to Orbit.Void,
        )
    }
    val motion = LocalMotionEnabled.current
    // Saveable: the hero is a LazyColumn item, disposed when scrolled away; scrolling back must
    // not replay the entrance.
    var revealed by rememberSaveable { mutableStateOf(false) }
    val reveal = remember { Animatable(if (motion && !revealed) 0f else 1f) }
    LaunchedEffect(Unit) {
        reveal.animateTo(1f, orbitTween(Motion.HeroRevealMs, motion))
        revealed = true
    }

    Box(modifier.fillMaxWidth()) {
        if (coverFile != null) {
            AsyncImage(
                coverFile,
                contentDescription = null,
                modifier = Modifier
                    .matchParentSize()
                    .blur(Dimens.BackdropBlur)
                    .graphicsLayer { alpha = Alpha.HeroScrim * reveal.value },
                contentScale = ContentScale.Crop,
            )
        }
        Box(Modifier.matchParentSize().background(tint))
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            header()
            Box(
                Modifier
                    .padding(top = Space.S)
                    .size(212.dp)
                    .graphicsLayer {
                        val p = reveal.value
                        alpha = p
                        val s = 0.94f + 0.06f * p
                        scaleX = s
                        scaleY = s
                    }
                    .border(BorderStroke(Dimens.Stroke, Orbit.Line)),
            ) {
                Cover(coverPath, Modifier.fillMaxSize(), bordered = false)
                CornerBrackets(Modifier.matchParentSize().padding(Space.S))
                coverOverlay()
            }
            Spacer(Modifier.height(Space.L))
            Text(
                title,
                modifier = Modifier.padding(horizontal = Space.Xxl),
                color = Orbit.Text,
                style = OrbitType.HeroTitle,
                textAlign = TextAlign.Center,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            if (subtitle != null) {
                Text(
                    subtitle,
                    modifier = Modifier
                        .heightIn(min = Dimens.Touch)
                        .then(if (onSubtitle != null) Modifier.clickable(onClick = onSubtitle) else Modifier)
                        .wrapContentHeight(Alignment.CenterVertically)
                        .padding(horizontal = Space.L, vertical = Space.Xs),
                    // Only a hero with a subtitle link reads the accent.
                    color = rememberReadableAccentOn(Orbit.Void, LocalAccent.current),
                    style = OrbitType.RowTitle,
                    textAlign = TextAlign.Center,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                meta.uppercase(),
                modifier = Modifier.padding(horizontal = Space.L),
                color = Orbit.TextDim,
                style = OrbitType.HeroMeta,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (quality != null) {
                Spacer(Modifier.height(Space.S))
                QualityPill(quality, accent = true)
            }
            Spacer(Modifier.height(Space.L))
            actions()
            Spacer(Modifier.height(Space.M))
        }
    }
}

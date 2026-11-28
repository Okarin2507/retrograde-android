package dev.okarin.rustygrade.ui.player

import android.graphics.RuntimeShader
import android.os.Build
import androidx.annotation.RequiresApi
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.ShaderBrush
import androidx.compose.ui.layout.ContentScale
import coil3.compose.AsyncImage
import dev.okarin.rustygrade.ui.components.LocalIsPlaying
import dev.okarin.rustygrade.ui.io
import dev.okarin.rustygrade.ui.LocalPowerSaveMode
import dev.okarin.rustygrade.ui.theme.Alpha
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalGlow
import dev.okarin.rustygrade.ui.theme.LocalMotionEnabled
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.withAlpha
import java.io.File
import uniffi.rustygrade_ffi.blurredCover

/** A slowly drifting radial light (AGSL, Android 13+). Output is premultiplied. */
private const val GlowAgsl = """
uniform float2 resolution;
uniform float time;
uniform float4 tint;

half4 main(float2 fragCoord) {
    float aspect = resolution.x / resolution.y;
    float2 uv = fragCoord / resolution;
    float2 p = float2(uv.x * aspect, uv.y);
    float2 a = float2((0.5 + 0.32 * sin(time * 0.21)) * aspect, 0.30 + 0.16 * cos(time * 0.17));
    float2 b = float2((0.5 + 0.34 * cos(time * 0.13 + 2.0)) * aspect, 0.75 + 0.14 * sin(time * 0.19 + 1.0));
    float da = distance(p, a);
    float db = distance(p, b);
    float g = 0.55 * exp(-da * da * 5.0) + 0.40 * exp(-db * db * 6.0);
    float alpha = clamp(g * 0.16, 0.0, 1.0);
    return half4(float4(tint.rgb * alpha, alpha));
}
"""

/**
 * Full-screen backdrop of the player: the cover blurred and darkened towards the void. Android 12+
 * blurs live with a render effect, Android 9-11 loads a tiny pre-blurred picture from the core.
 * On Android 13+ a decorative moving glow runs only while [fullyOpen], playing, resumed, with motion
 * on and not in battery saver; otherwise it stays a still frame (no frame callbacks at all).
 */
@Composable
fun PlayerBackdrop(coverPath: String?, fullyOpen: Boolean, modifier: Modifier = Modifier) {
    val glow = LocalGlow.current
    val tint = remember(glow) { Brush.verticalGradient(0f to glow.withAlpha(Alpha.ArtGlow), 0.7f to Orbit.Void) }
    val coverFile = remember(coverPath) { coverPath?.let(::File) }
    Box(modifier.fillMaxSize().background(Orbit.Void)) {
        // Tint underneath, so a track without a cover still gets the accent colour.
        Box(Modifier.fillMaxSize().background(tint))
        if (coverPath != null && coverFile != null) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                AsyncImage(coverFile, null, Modifier.fillMaxSize().blur(Dimens.BackdropBlur), contentScale = ContentScale.Crop)
            } else {
                LegacyBlurredCover(coverPath)
            }
        }
        Box(Modifier.fillMaxSize().background(BackdropScrim))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) MovingGlow(fullyOpen)
    }
}

/** Darkens the cover blur towards the void; the title bar chrome sits on its top. */
private val BackdropScrim = Brush.verticalGradient(
    0f to Orbit.Void.withAlpha(Alpha.BackdropTop),
    0.45f to Orbit.Void.withAlpha(Alpha.ScrimMid),
    0.82f to Orbit.Void.withAlpha(Alpha.ScrimDeep),
    1f to Orbit.Void,
)

@Composable
private fun LegacyBlurredCover(coverPath: String) {
    val blurred by produceState<File?>(initialValue = null, key1 = coverPath) {
        value = io { runCatching { blurredCover(coverPath) }.getOrNull() }?.let(::File)
    }
    blurred?.let { AsyncImage(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
}

@RequiresApi(Build.VERSION_CODES.TIRAMISU)
@Composable
private fun MovingGlow(fullyOpen: Boolean) {
    val glow = LocalGlow.current
    // LocalIsPlaying is already false while the app is not resumed.
    val active = fullyOpen && LocalIsPlaying.current && LocalMotionEnabled.current && !LocalPowerSaveMode.current

    val shader = remember { RuntimeShader(GlowAgsl) }
    val brush = remember(shader) { ShaderBrush(shader) }
    var time by remember { mutableFloatStateOf(0f) }
    // Frame callbacks exist only while every condition holds; the last time is kept for a still frame.
    LaunchedEffect(active) {
        if (active) {
            val startNanos = androidx.compose.runtime.withFrameNanos { it }
            val base = time
            while (true) {
                androidx.compose.runtime.withFrameNanos { now -> time = base + (now - startNanos) / 1_000_000_000f }
            }
        }
    }
    Canvas(Modifier.fillMaxSize()) {
        if (size.width <= 0f || size.height <= 0f) return@Canvas
        shader.setFloatUniform("resolution", size.width, size.height)
        shader.setFloatUniform("time", time)
        shader.setFloatUniform("tint", glow.red, glow.green, glow.blue, 1f)
        drawRect(brush)
    }
}

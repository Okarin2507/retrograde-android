package dev.okarin.rustygrade.media

import android.content.Context
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink

/**
 * An ExoPlayer with its own ReplayGain processor (a processor cannot be shared between sinks).
 * [usbDirect]: true when backed by [UsbDirectAudioSink] (userspace UAC1/UAC2 bit-perfect driver,
 * bypassing Android's AudioFlinger, mixer, EQ and ReplayGain completely).
 */
class PlayerHandle(val exo: ExoPlayer, val replayGain: ReplayGainAudioProcessor, val usbDirect: Boolean)

/**
 * Builds the music players. Standard players share one audio session so the equalizer covers both
 * during a crossfade; a Direct USB player swaps the sink for [UsbDirectAudioSink] so bit-perfect
 * PCM goes straight to our Rust USBFS driver.
 */
@UnstableApi
class PlayerFactory(private val context: Context, private val audioSessionId: Int) {

    fun create(
        handlesAudioFocus: Boolean,
        usbDirectSink: AudioSink? = null,
    ): PlayerHandle {
        val replayGain = ReplayGainAudioProcessor()
        val isUsbDirect = usbDirectSink != null
        val renderers = object : DefaultRenderersFactory(context) {
            override fun buildAudioSink(
                context: Context,
                enableFloatOutput: Boolean,
                enableAudioTrackPlaybackParams: Boolean,
            ): AudioSink = usbDirectSink ?: DefaultAudioSink.Builder(context)
                .setEnableFloatOutput(enableFloatOutput)
                .setEnableAudioOutputPlaybackParameters(enableAudioTrackPlaybackParams)
                .setAudioProcessors(arrayOf(replayGain))
                .build()
        }
        val player = ExoPlayer.Builder(context, renderers)
            .setAudioAttributes(MUSIC, handlesAudioFocus)
            // When our USB driver claims the DAC, Linux detaches snd-usb-audio which makes Android
            // broadcast AUDIO_BECOMING_NOISY; Direct USB handles physical unplug via UsbManager
            // ACTION_USB_DEVICE_DETACHED instead. Every other connect/disconnect pause (both modes)
            // comes from AudioRouteWatcher in PlaybackCoordinator.
            .setHandleAudioBecomingNoisy(!isUsbDirect)
            .setWakeMode(C.WAKE_MODE_LOCAL) // keep the CPU awake with the screen off
            .build()
        if (!isUsbDirect && EqualizerController.isUsableSession(audioSessionId)) {
            player.audioSessionId = audioSessionId
        }
        return PlayerHandle(player, replayGain, isUsbDirect)
    }

    companion object {
        val MUSIC: AudioAttributes = AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
            .build()
    }
}

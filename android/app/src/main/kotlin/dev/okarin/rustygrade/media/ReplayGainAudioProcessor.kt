package dev.okarin.rustygrade.media

import android.annotation.SuppressLint
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor.AudioFormat
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.roundToInt

/**
 * Scales PCM samples by a linear [gain]. Only used for ReplayGain *boosts* (> 1): attenuation goes
 * through `ExoPlayer.volume`, which takes effect exactly at the track boundary, while this
 * processor sits before the sink buffer, so a gain change reaches the speaker slightly late.
 *
 * Integer PCM is clamped (a boost never wraps around); float PCM is left to the sink to clip.
 */
@OptIn(UnstableApi::class)
class ReplayGainAudioProcessor : BaseAudioProcessor() {
    @Volatile
    private var linearGain: Float = 1f

    /** Linear gain; a non-finite or non-positive value is stored as unity (a pass-through). */
    var gain: Float
        get() = linearGain
        set(value) {
            linearGain = if (value.isFinite() && value > 0f) value else 1f
        }

    @SuppressLint("SwitchIntDef") // every other encoding is deliberately unsupported
    override fun onConfigure(inputAudioFormat: AudioFormat): AudioFormat {
        when (inputAudioFormat.encoding) {
            C.ENCODING_PCM_16BIT,
            C.ENCODING_PCM_24BIT,
            C.ENCODING_PCM_32BIT,
            C.ENCODING_PCM_FLOAT -> return inputAudioFormat
        }
        // 8-bit and big-endian PCM: leave the stream alone rather than fail playback.
        return AudioFormat.NOT_SET
    }

    @SuppressLint("SwitchIntDef")
    override fun queueInput(inputBuffer: ByteBuffer) {
        val size = inputBuffer.remaining()
        if (size == 0) return
        val out = replaceOutputBuffer(size)
        val g = gain
        if (!g.isFinite() || g <= 0f || g == 1f) { // the common case (and bit-perfect mode): no per-sample work
            out.put(inputBuffer)
            out.flip()
            return
        }
        val src = inputBuffer.duplicate().order(ByteOrder.LITTLE_ENDIAN)
        out.order(ByteOrder.LITTLE_ENDIAN)
        when (inputAudioFormat.encoding) {
            C.ENCODING_PCM_16BIT -> while (src.remaining() >= 2) {
                out.putShort((src.short * g).roundToInt().coerceIn(-32768, 32767).toShort())
            }
            C.ENCODING_PCM_24BIT -> while (src.remaining() >= 3) {
                val b0 = src.get().toInt() and 0xFF
                val b1 = src.get().toInt() and 0xFF
                val b2 = src.get().toInt() // sign-carrying byte
                val sample = (b2 shl 16) or (b1 shl 8) or b0
                val scaled = (sample * g.toDouble()).roundToInt().coerceIn(-8_388_608, 8_388_607)
                out.put(scaled.toByte())
                out.put((scaled shr 8).toByte())
                out.put((scaled shr 16).toByte())
            }
            C.ENCODING_PCM_32BIT -> while (src.remaining() >= 4) {
                val scaled = (src.int * g.toDouble()).roundToInt() // saturates at the Int range by itself
                out.putInt(scaled)
            }
            C.ENCODING_PCM_FLOAT -> while (src.remaining() >= 4) {
                out.putFloat(src.float * g)
            }
        }
        inputBuffer.position(inputBuffer.limit())
        out.flip()
    }
}

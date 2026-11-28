package dev.okarin.rustygrade.media

import android.util.Log
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.audio.AudioSink
import java.nio.ByteBuffer
import uniffi.rustygrade_ffi.UsbAudioDriver
import uniffi.rustygrade_ffi.UsbDriverException
import uniffi.rustygrade_ffi.UsbPcmEncoding
import uniffi.rustygrade_ffi.UsbStreamSpecDto

/**
 * Custom Media3 [AudioSink] that streams decoded PCM directly to our native UAC1/UAC2 USBFS
 * driver ([UsbAudioDriver]), bypassing Android's `AudioTrack`, AudioFlinger, system mixer, and
 * resampler completely.
 *
 * - Accepts 16-bit, 24-bit, 32-bit integer PCM and 32-bit float PCM directly.
 * - Reconfigures the DAC hardware clock on sample-rate changes without rebuilding ExoPlayer.
 * - Keeps gapless same-rate transitions unbroken.
 * - Never applies software volume scaling (bit-perfect PCM intact).
 */
@UnstableApi
class UsbDirectAudioSink(
    private val driverProvider: () -> UsbAudioDriver?,
    private val sourceBitsProvider: () -> Int,
    private val onStreamConfigured: (UsbStreamSpecDto) -> Unit,
    private val onStreamError: (Throwable) -> Unit,
) : AudioSink {

    private var listener: AudioSink.Listener? = null
    private var audioAttributes: AudioAttributes = AudioAttributes.DEFAULT
    private var playbackParameters: PlaybackParameters = PlaybackParameters.DEFAULT
    private var skipSilenceEnabled: Boolean = false

    private var currentFormat: Format? = null
    private var configured: Boolean = false
    private var playing: Boolean = false
    /**
     * The driver whose iso thread runs (our last [UsbAudioDriver.start] succeeded on it and nothing
     * stopped it since). `start()` takes the driver's EP0 `control` lock, which a hardware-volume transfer can
     * hold for ~1 s: calling it once per buffer would stall the audio thread behind that transfer.
     */
    private var startedDriver: UsbAudioDriver? = null
    private var inputEnded: Boolean = false

    private var startMediaTimeUs: Long = C.TIME_UNSET
    private var startMediaTimeUsNeedsSync: Boolean = false
    private var lastReportedPositionUs: Long = 0L

    private var scratch = ByteArray(16_384)

    override fun setListener(listener: AudioSink.Listener) {
        this.listener = listener
    }

    override fun supportsFormat(format: Format): Boolean =
        getFormatSupport(format) != AudioSink.SINK_FORMAT_UNSUPPORTED

    override fun getFormatSupport(format: Format): Int {
        if (format.sampleMimeType != MimeTypes.AUDIO_RAW) {
            return AudioSink.SINK_FORMAT_UNSUPPORTED
        }
        if (toUsbPcmEncoding(format.pcmEncoding) == null) {
            return AudioSink.SINK_FORMAT_UNSUPPORTED
        }
        val rate = format.sampleRate
        val rateOk = withDriver { d ->
            !d.isDisconnected() && (rate == Format.NO_VALUE || rate <= 0 || d.supportsSampleRate(rate.toUInt()))
        }
        if (rateOk != true) return AudioSink.SINK_FORMAT_UNSUPPORTED
        val channels = format.channelCount
        if (channels != Format.NO_VALUE && (channels <= 0 || channels > 8)) {
            return AudioSink.SINK_FORMAT_UNSUPPORTED
        }
        return AudioSink.SINK_FORMAT_SUPPORTED_DIRECTLY
    }

    @Deprecated("Overrides deprecated Media3 AudioSink overload")
    override fun configure(inputFormat: Format, specifiedBufferSize: Int, outputChannels: IntArray?) {
        configureInternal(inputFormat)
    }

    override fun configure(config: AudioSink.AudioSinkConfig) {
        configureInternal(config.format)
    }

    private fun configureInternal(inputFormat: Format) {
        val encoding = toUsbPcmEncoding(inputFormat.pcmEncoding)
            ?: throw AudioSink.ConfigurationException("Unsupported PCM encoding ${inputFormat.pcmEncoding}", inputFormat)
        val sampleRate = inputFormat.sampleRate
        val channels = inputFormat.channelCount
        if (sampleRate <= 0 || channels <= 0) {
            throw AudioSink.ConfigurationException(
                "Invalid audio format ${sampleRate}Hz ${channels}ch",
                inputFormat,
            )
        }
        val driver = driverProvider() ?: run {
            // The session closed under us (unplug race): a USB fallback, not an unreadable track.
            val e = IllegalStateException("USB DAC driver is not open")
            onStreamError(e)
            throw AudioSink.ConfigurationException(UsbStreamFailure(e), inputFormat)
        }

        val sourceBits = resolveSourceBits(inputFormat.pcmEncoding, sourceBitsProvider())
        startedDriver = null // a reconfigured stream may come back without its thread
        val spec = try {
            driver.configure(
                sampleRate = sampleRate.toUInt(),
                channels = channels.toUByte(),
                pcmEncoding = encoding,
                sourceBits = sourceBits.toUByte(),
            )
        } catch (e: Exception) {
            // UsbDriverException, or IllegalStateException when the session was closed under us.
            onStreamError(e)
            throw AudioSink.ConfigurationException(UsbStreamFailure(e), inputFormat)
        }

        currentFormat = inputFormat
        configured = true
        inputEnded = false
        // If Rust created a fresh stream (new sample rate / altsetting), reset our clock anchor
        // so the first buffer of the new stream anchors cleanly at its presentation timestamp.
        if ((withDriver { it.playedPositionUs() } ?: 0uL) == 0uL) {
            startMediaTimeUs = C.TIME_UNSET
            startMediaTimeUsNeedsSync = false
            lastReportedPositionUs = 0L
        }

        Log.i(
            TAG,
            "Configured USB DAC bit-perfect: ${spec.uacVersion} ${spec.sampleRate} Hz " +
                "${spec.bitResolution}-bit (${spec.subslotBytes}B/slot) ${spec.channels}ch " +
                "${spec.syncMode} fb=${spec.hasFeedback}",
        )
        onStreamConfigured(spec)

        if (playing) {
            try {
                driver.start()
                startedDriver = driver
            } catch (e: Exception) {
                onStreamError(e)
                throw AudioSink.ConfigurationException(UsbStreamFailure(e), inputFormat)
            }
        }
    }

    override fun play() {
        playing = true
        if (configured) {
            val driver = driverProvider() ?: return
            try {
                driver.start()
                startedDriver = driver
            } catch (e: Exception) {
                onStreamError(e)
            }
        }
    }

    override fun handleDiscontinuity() {
        startMediaTimeUsNeedsSync = true
    }

    override fun handleBuffer(
        buffer: ByteBuffer,
        presentationTimeUs: Long,
        encodedAccessUnitCount: Int,
    ): Boolean {
        val format = currentFormat ?: return true
        val savedPos = buffer.position()
        try {
            return writeBuffer(buffer, presentationTimeUs)
        } catch (e: Exception) {
            // UsbDriverException, or IllegalStateException once the driver is gone or destroyed:
            // [onStreamError] marks it as a USB fallback so the player does not skip the track.
            buffer.position(savedPos)
            onStreamError(e)
            // WriteException's constructor sets no cause, so initCause can attach the marker.
            throw AudioSink.WriteException(-1, format, false).apply { initCause(UsbStreamFailure(e)) }
        }
    }

    private fun writeBuffer(buffer: ByteBuffer, presentationTimeUs: Long): Boolean {
        val driver = driverProvider()
        if (driver == null || driver.isDisconnected()) throw IllegalStateException("USB DAC disconnected")

        val playedUs = driver.playedPositionUs().toLong()
        if (startMediaTimeUs == C.TIME_UNSET) {
            startMediaTimeUs = maxOf(0L, presentationTimeUs)
            lastReportedPositionUs = startMediaTimeUs
        } else if (startMediaTimeUsNeedsSync) {
            val expectedUs = startMediaTimeUs + playedUs
            startMediaTimeUs += (presentationTimeUs - expectedUs)
            startMediaTimeUsNeedsSync = false
        }

        if (playing && startedDriver !== driver) {
            driver.start()
            startedDriver = driver
        }

        val remaining = buffer.remaining()
        if (remaining == 0) return true

        val toRead = remaining.coerceAtMost(MAX_WRITE_CHUNK_BYTES)
        if (scratch.size < toRead) {
            scratch = ByteArray(toRead)
        }
        val slice = if (toRead == scratch.size) scratch else ByteArray(toRead)
        val savedPos = buffer.position()
        buffer.get(slice, 0, toRead)

        val consumed = driver.writePcm(slice).toInt()
        buffer.position(savedPos + consumed)
        return !buffer.hasRemaining()
    }

    override fun playToEndOfStream() {
        inputEnded = true
        withDriver { it.markEndOfStream() }
    }

    override fun isEnded(): Boolean =
        !configured || (inputEnded && (withDriver { it.isDrained() } ?: true))

    override fun hasPendingData(): Boolean =
        configured && (withDriver { it.hasPendingData() } ?: false)

    override fun getCurrentPositionUs(sourceEnded: Boolean): Long {
        if (!configured || startMediaTimeUs == C.TIME_UNSET) {
            return AudioSink.CURRENT_POSITION_NOT_SET
        }
        val playedUs = withDriver { it.playedPositionUs() }?.toLong() ?: 0L
        val pos = (startMediaTimeUs + playedUs).coerceAtLeast(lastReportedPositionUs)
        lastReportedPositionUs = pos
        return pos
    }

    override fun setPlaybackParameters(playbackParameters: PlaybackParameters) {
        // Bit-perfect playback runs strictly at 1.0x native DAC clock (no resampling / time-stretch).
        this.playbackParameters = PlaybackParameters.DEFAULT
    }

    override fun getPlaybackParameters(): PlaybackParameters = playbackParameters

    override fun setSkipSilenceEnabled(skipSilenceEnabled: Boolean) {
        this.skipSilenceEnabled = false
    }

    override fun getSkipSilenceEnabled(): Boolean = skipSilenceEnabled

    override fun setAudioAttributes(audioAttributes: AudioAttributes) {
        this.audioAttributes = audioAttributes
    }

    override fun getAudioAttributes(): AudioAttributes = audioAttributes

    override fun setAudioSessionId(audioSessionId: Int) = Unit

    override fun setAuxEffectInfo(auxEffectInfo: androidx.media3.common.AuxEffectInfo) = Unit

    override fun getAudioTrackBufferSizeUs(): Long =
        withDriver { it.bufferDurationUs() }?.toLong()?.takeIf { it > 0L } ?: C.TIME_UNSET

    override fun enableTunnelingV21() = Unit

    override fun disableTunneling() = Unit

    override fun setVolume(volume: Float) {
        // Bit-perfect PCM: never apply software gain scaling. Hardware volume lives on the DAC's
        // UAC Feature Unit (UsbDacController.setHardwareVolume).
    }

    override fun pause() {
        playing = false
        startedDriver = null
        withDriver { it.pause() }
    }

    override fun flush() {
        withDriver { it.flush() }
        startMediaTimeUs = C.TIME_UNSET
        startMediaTimeUsNeedsSync = false
        lastReportedPositionUs = 0L
        inputEnded = false
    }

    override fun reset() {
        flush()
        playing = false
        startedDriver = null
        configured = false
        currentFormat = null
    }

    override fun release() {
        reset()
    }

    /**
     * Runs [block] on the active driver. Null when there is none, or when the session closed under
     * us: a destroyed UniFFI object throws [IllegalStateException], a shut-down driver
     * [UsbDriverException]. Neither may escape into ExoPlayer from a query or transport call.
     */
    private inline fun <T> withDriver(block: (UsbAudioDriver) -> T): T? {
        val driver = driverProvider() ?: return null
        return try {
            block(driver)
        } catch (_: IllegalStateException) {
            null
        } catch (_: UsbDriverException) {
            null
        }
    }

    private companion object {
        const val TAG = "RustyUsbSink"
        const val MAX_WRITE_CHUNK_BYTES = 32_768
    }
}

/**
 * Cause attached to every [AudioSink] exception [UsbDirectAudioSink] throws for a driver failure, so
 * the player's error handler can tell a USB stream failure (handled by `onStreamError`) from an
 * unreadable track.
 */
class UsbStreamFailure(cause: Throwable) : Exception(cause.message, cause)

/** Maps Media3 `C.ENCODING_PCM_*` to the Rust USB driver's [UsbPcmEncoding]. */
@OptIn(UnstableApi::class)
internal fun toUsbPcmEncoding(pcmEncoding: Int): UsbPcmEncoding? = when (pcmEncoding) {
    C.ENCODING_PCM_16BIT -> UsbPcmEncoding.PCM16_LE
    C.ENCODING_PCM_24BIT -> UsbPcmEncoding.PCM24_LE
    C.ENCODING_PCM_24BIT_BIG_ENDIAN -> UsbPcmEncoding.PCM24_BE
    C.ENCODING_PCM_32BIT -> UsbPcmEncoding.PCM32_LE
    C.ENCODING_PCM_32BIT_BIG_ENDIAN -> UsbPcmEncoding.PCM32_BE
    C.ENCODING_PCM_FLOAT -> UsbPcmEncoding.PCM_FLOAT32_LE
    else -> null
}

/**
 * Resolves the target bit depth to request from the DAC's alternate settings:
 * - Explicit integer encodings dictate their own bit depth (16 / 24 / 32).
 * - Float32 (used by MediaCodec for FLAC) uses the track's scanned `bitsPerSample` when known
 *   (so 16-bit FLAC can pick a 16-bit altsetting and 24-bit FLAC picks 24/32-bit), defaulting to 24.
 */
@OptIn(UnstableApi::class)
internal fun resolveSourceBits(pcmEncoding: Int, trackBitsPerSample: Int): Int = when (pcmEncoding) {
    C.ENCODING_PCM_16BIT -> 16
    C.ENCODING_PCM_24BIT, C.ENCODING_PCM_24BIT_BIG_ENDIAN -> 24
    C.ENCODING_PCM_32BIT, C.ENCODING_PCM_32BIT_BIG_ENDIAN -> 32
    C.ENCODING_PCM_FLOAT -> if (trackBitsPerSample > 0) trackBitsPerSample.coerceIn(16, 32) else 24
    else -> 16
}

package dev.okarin.rustygrade.storage

import android.content.Context
import android.media.AudioFormat
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.net.Uri
import android.util.Log
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import uniffi.rustygrade_ffi.R128BatchScanner
import uniffi.rustygrade_ffi.ReplayGainUpdateDto
import uniffi.rustygrade_ffi.RustygradeEngine
import uniffi.rustygrade_ffi.TrackInfo

data class ReplayGainScanSummary(
    val scanned: Int,
    val skipped: Int,
    val albums: Int,
)

/**
 * In-process EBU R128 / ITU-R BS.1770-4 ReplayGain 2.0 scanner (`-18.0 LUFS` target).
 *
 * Decodes local files and SAF `content://` tracks through Android's [MediaExtractor] + [MediaCodec]
 * into raw PCM (16-bit, packed 24-bit, 32-bit integer or float), feeds chunks into Rust [R128BatchScanner], computes
 * per-track and pooled album gain/peak, and persists results into `library.json`.
 */
class ReplayGainScanner(
    private val context: Context,
    private val engine: RustygradeEngine,
) {
    /**
     * Scans [tracks] grouped by album (so album gain pools loudness across every track of each
     * album) and saves each completed album incrementally into [engine].
     */
    suspend fun scanTracks(
        tracks: List<TrackInfo>,
        onProgress: (done: Int, total: Int, currentTrack: String) -> Unit,
    ): ReplayGainScanSummary = withContext(Dispatchers.IO) {
        if (tracks.isEmpty()) return@withContext ReplayGainScanSummary(0, 0, 0)
        val byAlbum = tracks.groupBy { it.album }
        var done = 0
        var scanned = 0
        var skipped = 0
        var albumsUpdated = 0

        for ((_, albumTracks) in byAlbum) {
            currentCoroutineContext().ensureActive()
            val batch = R128BatchScanner()
            try {
                for (track in albumTracks) {
                    currentCoroutineContext().ensureActive()
                    onProgress(done, tracks.size, track.title)
                    val ok = decodeIntoScanner(track, batch)
                    if (ok) scanned++ else skipped++
                    done++
                }
                val summary = batch.finishAlbum()
                if (summary.tracks.isNotEmpty()) {
                    val updates = summary.tracks.map { t ->
                        ReplayGainUpdateDto(
                            trackId = t.trackId,
                            trackGainDb = t.gainDb,
                            trackPeak = t.peak,
                            albumGainDb = summary.albumGainDb,
                            albumPeak = summary.albumPeak,
                        )
                    }
                    engine.applyReplayGainResults(updates)
                    albumsUpdated++
                }
            } finally {
                batch.destroy()
            }
        }
        onProgress(done, tracks.size, "")
        ReplayGainScanSummary(scanned = scanned, skipped = skipped, albums = albumsUpdated)
    }

    private suspend fun decodeIntoScanner(track: TrackInfo, batch: R128BatchScanner): Boolean {
        val extractor = MediaExtractor()
        var codec: MediaCodec? = null
        try {
            if (track.playUri.startsWith("content://")) {
                extractor.setDataSource(context, Uri.parse(track.playUri), null)
            } else {
                val file = File(track.filePath)
                if (!file.isFile) return false
                extractor.setDataSource(file.absolutePath)
            }

            var audioTrackIdx = -1
            var format: MediaFormat? = null
            for (i in 0 until extractor.trackCount) {
                val fmt = extractor.getTrackFormat(i)
                val mime = fmt.getString(MediaFormat.KEY_MIME) ?: continue
                if (mime.startsWith("audio/")) {
                    audioTrackIdx = i
                    format = fmt
                    break
                }
            }
            if (audioTrackIdx < 0 || format == null) return false
            val mime = format.getString(MediaFormat.KEY_MIME) ?: return false
            extractor.selectTrack(audioTrackIdx)

            codec = MediaCodec.createDecoderByType(mime)
            codec.configure(format, null, null, 0)
            codec.start()

            val info = MediaCodec.BufferInfo()
            var inputDone = false
            var outputDone = false
            var started = false
            var pcmEncoding = AudioFormat.ENCODING_PCM_16BIT
            var pcmChunk = ByteArray(0)

            var sampleRate = format.getInteger(MediaFormat.KEY_SAMPLE_RATE)
            var channels = format.getInteger(MediaFormat.KEY_CHANNEL_COUNT)

            while (!outputDone) {
                currentCoroutineContext().ensureActive()

                if (!inputDone) {
                    val inIdx = codec.dequeueInputBuffer(TIMEOUT_US)
                    if (inIdx >= 0) {
                        val inBuf = codec.getInputBuffer(inIdx)
                        if (inBuf != null) {
                            val read = extractor.readSampleData(inBuf, 0)
                            if (read < 0) {
                                codec.queueInputBuffer(inIdx, 0, 0, 0L, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                                inputDone = true
                            } else {
                                val pts = extractor.sampleTime.coerceAtLeast(0L)
                                codec.queueInputBuffer(inIdx, 0, read, pts, 0)
                                extractor.advance()
                            }
                        }
                    }
                }

                val outIdx = codec.dequeueOutputBuffer(info, TIMEOUT_US)
                when {
                    outIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        val outFmt = codec.outputFormat
                        if (outFmt.containsKey(MediaFormat.KEY_SAMPLE_RATE)) {
                            sampleRate = outFmt.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                        }
                        if (outFmt.containsKey(MediaFormat.KEY_CHANNEL_COUNT)) {
                            channels = outFmt.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                        }
                        pcmEncoding = if (outFmt.containsKey(MediaFormat.KEY_PCM_ENCODING)) {
                            outFmt.getInteger(MediaFormat.KEY_PCM_ENCODING)
                        } else {
                            AudioFormat.ENCODING_PCM_16BIT
                        }
                        // Fed as anything else, the samples would be misread: skip the track instead.
                        if (pcmEncoding !in SUPPORTED_PCM_ENCODINGS) {
                            Log.w(TAG, "R128 skipped '${track.title}': unsupported PCM encoding $pcmEncoding")
                            return false
                        }
                    }
                    outIdx >= 0 -> {
                        val outBuf = codec.getOutputBuffer(outIdx)
                        if (outBuf != null && info.size > 0) {
                            if (!started) {
                                batch.beginTrack(
                                    trackId = track.id,
                                    sampleRate = sampleRate.coerceAtLeast(8_000).toUInt(),
                                    channels = channels.coerceIn(1, 8).toUByte(),
                                )
                                started = true
                            }
                            outBuf.position(info.offset)
                            outBuf.limit(info.offset + info.size)
                            if (pcmChunk.size != info.size) {
                                pcmChunk = ByteArray(info.size)
                            }
                            outBuf.get(pcmChunk)
                            when (pcmEncoding) {
                                AudioFormat.ENCODING_PCM_FLOAT -> batch.feedF32le(pcmChunk)
                                ENCODING_PCM_24BIT_PACKED -> batch.feedS24le(pcmChunk)
                                ENCODING_PCM_32BIT -> batch.feedS32le(pcmChunk)
                                else -> batch.feedS16le(pcmChunk)
                            }
                        }
                        codec.releaseOutputBuffer(outIdx, false)
                        if ((info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                            outputDone = true
                        }
                    }
                }
            }

            if (!started) return false
            val res = batch.finishTrack() ?: return false
            Log.i(
                TAG,
                "R128 '${track.title}': ${String.format("%.2f", res.integratedLufs)} LUFS -> " +
                    "gain=${String.format("%+.2f", res.gainDb)} dB, peak=${String.format("%.4f", res.peak)}",
            )
            return true
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "R128 decode failed for '${track.title}' (${track.filePath}): ${e.message}")
            return false
        } finally {
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            runCatching { extractor.release() }
        }
    }

    private companion object {
        const val TAG = "RustyR128"
        const val TIMEOUT_US = 10_000L

        // `AudioFormat.ENCODING_PCM_24BIT_PACKED` / `ENCODING_PCM_32BIT` (API 31; minSdk is 28):
        // decoders of hi-res FLAC/WAV may output them on Android 12+.
        const val ENCODING_PCM_24BIT_PACKED = 21
        const val ENCODING_PCM_32BIT = 22
        val SUPPORTED_PCM_ENCODINGS = setOf(
            AudioFormat.ENCODING_PCM_16BIT,
            AudioFormat.ENCODING_PCM_FLOAT,
            ENCODING_PCM_24BIT_PACKED,
            ENCODING_PCM_32BIT,
        )
    }
}

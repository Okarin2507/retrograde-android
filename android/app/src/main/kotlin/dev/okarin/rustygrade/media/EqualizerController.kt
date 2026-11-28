package dev.okarin.rustygrade.media

import android.media.AudioManager
import android.media.audiofx.DynamicsProcessing
import android.util.Log
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import uniffi.rustygrade_ffi.EqPlan
import uniffi.rustygrade_ffi.eqBandCutoffs

/**
 * 10-band parametric EQ on the player's audio session via `DynamicsProcessing` (API 28+).
 *
 * The effect is created once. Switching presets only rewrites band gains and the input pre-amp on
 * the live instance, so there is no stutter from re-creating it.
 */
class EqualizerController(private val audioSessionId: Int) {
    private var effect: DynamicsProcessing? = null
    private val cutoffs: List<Float> = eqBandCutoffs()

    init {
        effect = if (!isUsableSession(audioSessionId)) {
            // Session 0 is the global output mix: an effect there would equalize every app.
            Log.w(TAG, "No audio session: playback has no EQ")
            null
        } else try {
            val config = DynamicsProcessing.Config.Builder(
                DynamicsProcessing.VARIANT_FAVOR_FREQUENCY_RESOLUTION,
                CHANNELS,
                /* preEqInUse = */ false, 0,
                /* mbcInUse = */ false, 0,
                /* postEqInUse = */ true, cutoffs.size,
                /* limiterInUse = */ false,
            ).build()
            DynamicsProcessing(0, audioSessionId, config).also { it.enabled = false }
        } catch (e: Exception) {
            // Some OEM builds lack the effect: playback simply has no EQ.
            Log.w(TAG, "DynamicsProcessing unavailable", e)
            null
        }
    }

    /** Applies the plan live. A flat plan turns the effect off so it costs nothing. */
    fun apply(plan: EqPlan, enabled: Boolean) {
        val fx = effect ?: return
        try {
            val flat = plan.bandGainsDb.all { it == 0f } && plan.preampDb == 0f
            if (!enabled || flat) {
                fx.enabled = false
                return
            }
            // `postEqInUse` only allocates the stage: the stage itself has to be enabled too, or the
            // bands do nothing and only the (negative) pre-amp would be heard.
            val eq = DynamicsProcessing.Eq(true, true, cutoffs.size)
            // One band per cutoff, even if the plan is shorter or longer: a missing gain is flat.
            for (band in cutoffs.indices) {
                eq.setBand(band, DynamicsProcessing.EqBand(true, cutoffs[band], plan.bandGainsDb.getOrElse(band) { 0f }))
            }
            fx.setPostEqAllChannelsTo(eq)
            fx.setInputGainAllChannelsTo(plan.preampDb)
            fx.enabled = true
        } catch (e: Exception) {
            Log.w(TAG, "Could not apply EQ plan", e)
        }
    }

    fun release() {
        try {
            effect?.release()
        } catch (_: Exception) {
        }
        effect = null
    }

    companion object {
        /** A real audio session: not `AudioManager.ERROR` and not 0 (the global mix). */
        @OptIn(UnstableApi::class)
        fun isUsableSession(id: Int): Boolean = id != AudioManager.ERROR && id != C.AUDIO_SESSION_ID_UNSET

        private const val TAG = "RustyEq"
        private const val CHANNELS = 2
    }
}

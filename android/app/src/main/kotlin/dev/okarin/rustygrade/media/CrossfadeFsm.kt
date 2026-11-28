package dev.okarin.rustygrade.media

/** What the crossfade state machine drives (two ExoPlayers in the service; a fake in tests). */
interface CrossfadeHost {
    /** Starts the spare player on the next track, silent. */
    fun startSpare()

    /** Volumes (0..1 fade gains) for the outgoing and the incoming player. */
    fun applyFade(fadeOut: Double, fadeIn: Double)

    /** The fade finished: the spare becomes the player. */
    fun commit()

    /** The fade was interrupted: drop the spare and restore the outgoing player's volume. */
    fun abort()
}

/**
 * The timing of an equal-power crossfade, free of Android types.
 *
 * Feed it the playback position as it changes ([onProgress]) and call [tick] while [isFading].
 * The fade starts once, when the outgoing track has [fadeSeconds] left and a fade applies; anything
 * that changes what comes next (skip, seek, queue edit, pause) must [cancel] it.
 */
class CrossfadeFsm(
    private val host: CrossfadeHost,
    private val gains: (t: Double) -> Pair<Double, Double>,
) {
    private var startedAtMs = 0L
    private var lengthMs = 0L
    private var spent = false

    var isFading = false
        private set

    /** Whether the position is close enough to the end that the fade should begin. */
    fun onProgress(
        positionMs: Long,
        durationMs: Long,
        applies: Boolean,
        fadeSeconds: Int,
        playing: Boolean,
        nowMs: Long,
    ) {
        if (isFading || spent || !playing || !applies || fadeSeconds <= 0 || durationMs <= 0) return
        val fadeMs = fadeSeconds * 1000L
        if (durationMs - positionMs > fadeMs) return
        isFading = true
        lengthMs = fadeMs.coerceAtMost((durationMs - positionMs).coerceAtLeast(1))
        startedAtMs = nowMs
        host.startSpare()
        host.applyFade(1.0, 0.0)
    }

    /** Advances the fade; commits when it reaches the end. */
    fun tick(nowMs: Long) {
        if (!isFading) return
        val t = ((nowMs - startedAtMs).toDouble() / lengthMs).coerceIn(0.0, 1.0)
        val (out, inn) = gains(t)
        host.applyFade(out, inn)
        if (t >= 1.0) {
            isFading = false
            spent = true
            host.commit()
        }
    }

    /** The outgoing player ran out before the fade ended: complete it now. */
    fun finishNow() {
        if (!isFading) return
        isFading = false
        spent = true
        host.commit()
    }

    /** Stops a running fade (skip, seek, pause, queue edit). The next position update may restart it. */
    fun cancel() {
        if (!isFading) return
        isFading = false
        host.abort()
    }

    /**
     * The fade cannot work for this track (the incoming player failed or never became ready): stop
     * it and do not start it again until [reset]. Unlike [cancel], a position update will not retry.
     */
    fun abandon() {
        spent = true
        cancel()
    }

    /**
     * The incoming player became ready [nowMs] after the fade began: restart the curve from here
     * and shorten it by the time spent waiting, so it still ends with the outgoing track.
     */
    fun rebase(nowMs: Long) {
        if (!isFading) return
        val waited = (nowMs - startedAtMs).coerceAtLeast(0L)
        lengthMs = (lengthMs - waited).coerceAtLeast(1L)
        startedAtMs = nowMs
    }

    /** A new track is playing: a fade may start again for it. */
    fun reset() {
        isFading = false
        spent = false
    }
}

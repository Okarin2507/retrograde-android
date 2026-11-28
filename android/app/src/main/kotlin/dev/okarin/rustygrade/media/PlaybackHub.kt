package dev.okarin.rustygrade.media

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import uniffi.rustygrade_ffi.PlayerState
import uniffi.rustygrade_ffi.UsbHardwareVolumeDto

/**
 * Playback state for the UI. The media service writes it; Compose reads it. Transport commands go
 * the other way through a `MediaController` ([PlaybackClient]), never through this object.
 */
object PlaybackHub {
    private val _state = MutableStateFlow<PlayerState?>(null)
    val state: StateFlow<PlayerState?> = _state

    private val _isPlaying = MutableStateFlow(false)
    val isPlaying: StateFlow<Boolean> = _isPlaying

    private val _positionMs = MutableStateFlow(0L)
    val positionMs: StateFlow<Long> = _positionMs

    private val _durationMs = MutableStateFlow(0L)
    val durationMs: StateFlow<Long> = _durationMs

    private val _usb = MutableStateFlow<UsbStatus>(UsbStatus.Off)
    val usb: StateFlow<UsbStatus> = _usb

    private val _usbVolume = MutableStateFlow<UsbHardwareVolumeDto?>(null)
    val usbVolume: StateFlow<UsbHardwareVolumeDto?> = _usbVolume

    private val _error = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = _error

    internal fun setState(s: PlayerState?) { _state.value = s }
    private var anchorPositionMs = 0L
    private var anchorAtMs = 0L

    internal fun setPlaying(v: Boolean) {
        // Re-anchor so the estimate does not jump when playback starts or stops.
        anchorPositionMs = estimatedPositionMs()
        anchorAtMs = android.os.SystemClock.elapsedRealtime()
        _isPlaying.value = v
    }

    internal fun setPosition(positionMs: Long, durationMs: Long) {
        anchorPositionMs = positionMs
        anchorAtMs = android.os.SystemClock.elapsedRealtime()
        _positionMs.value = positionMs
        _durationMs.value = durationMs
    }

    /**
     * Where playback is right now, extrapolated from the last once-a-second report. Lets a lyrics view
     * highlight the right line at 150 ms resolution without the player being polled that often.
     */
    fun estimatedPositionMs(): Long {
        val base = anchorPositionMs
        val estimate = if (_isPlaying.value) base + (android.os.SystemClock.elapsedRealtime() - anchorAtMs) else base
        val duration = _durationMs.value
        return if (duration > 0) estimate.coerceIn(0, duration) else estimate.coerceAtLeast(0)
    }
    internal fun setUsb(v: UsbStatus) { _usb.value = v }
    internal fun setUsbVolume(v: UsbHardwareVolumeDto?) { _usbVolume.value = v }
    internal fun setError(v: String?) { _error.value = v }
}

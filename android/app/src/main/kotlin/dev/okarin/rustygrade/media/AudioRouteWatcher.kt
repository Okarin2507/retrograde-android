package dev.okarin.rustygrade.media

import android.content.Context
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Handler
import android.os.SystemClock
import android.util.Log

/**
 * Reports when a headphone-class audio output (USB DAC/headset, wired, Bluetooth A2DP/LE, hearing
 * aid, line out) connects or disconnects, so playback can pause instead of moving to the new route.
 *
 * - Devices already present when [start] runs (the callback's initial `onAudioDevicesAdded`) are
 *   ignored: only ids not seen before count as a connect.
 * - USB-type events are dropped while [suppressUsb] says the change is the app's own DAC
 *   claim/release/kernel reconnect (Direct USB), not the user plugging or unplugging something.
 * - Bursts (Bluetooth flapping, LE audio left+right buds) collapse into one [onChange] per
 *   [DEBOUNCE_MS].
 *
 * Main thread only.
 */
class AudioRouteWatcher(
    context: Context,
    private val handler: Handler,
    private val suppressUsb: () -> Boolean,
    private val onChange: (connected: Boolean, device: AudioDeviceInfo) -> Unit,
) {
    private val audioManager = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val known = HashSet<Int>()
    private var lastFiredAt = Long.MIN_VALUE / 2
    private var registered = false

    private val callback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) {
            var hit: AudioDeviceInfo? = null
            for (d in addedDevices) {
                if (!d.isSink || !known.add(d.id)) continue // initial snapshot / duplicate
                if (relevant(d)) hit = hit ?: d
            }
            hit?.let { fire(connected = true, device = it) }
        }

        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) {
            var hit: AudioDeviceInfo? = null
            for (d in removedDevices) {
                if (!d.isSink) continue
                known.remove(d.id)
                if (relevant(d)) hit = hit ?: d
            }
            hit?.let { fire(connected = false, device = it) }
        }
    }

    fun start() {
        if (registered) return
        registered = true
        known.clear()
        audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).forEach { known.add(it.id) }
        audioManager.registerAudioDeviceCallback(callback, handler)
    }

    fun stop() {
        if (!registered) return
        registered = false
        runCatching { audioManager.unregisterAudioDeviceCallback(callback) }
        known.clear()
    }

    private fun relevant(d: AudioDeviceInfo): Boolean {
        val type = d.type
        if (type !in WATCHED_TYPES) return false
        if (type in USB_TYPES && suppressUsb()) {
            Log.i(TAG, "Ignoring own USB route change: ${d.productName} (type=$type)")
            return false
        }
        return true
    }

    private fun fire(connected: Boolean, device: AudioDeviceInfo) {
        val now = SystemClock.elapsedRealtime()
        if (now - lastFiredAt < DEBOUNCE_MS) return
        lastFiredAt = now
        Log.i(TAG, "Output ${if (connected) "connected" else "disconnected"}: ${device.productName} (type=${device.type})")
        onChange(connected, device)
    }

    private companion object {
        const val TAG = "RustyRoute"
        const val DEBOUNCE_MS = 1500L

        // API 31+ constants by value (minSdk 28): TYPE_BLE_HEADSET = 26, TYPE_BLE_SPEAKER = 27.
        const val TYPE_BLE_HEADSET = 26
        const val TYPE_BLE_SPEAKER = 27

        val USB_TYPES = setOf(AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET)

        val WATCHED_TYPES = USB_TYPES + setOf(
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
            AudioDeviceInfo.TYPE_HEARING_AID,
            AudioDeviceInfo.TYPE_LINE_ANALOG,
            AudioDeviceInfo.TYPE_LINE_DIGITAL,
            TYPE_BLE_HEADSET,
            TYPE_BLE_SPEAKER,
        )
    }
}

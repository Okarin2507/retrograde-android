package dev.okarin.rustygrade.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.media.UsbStatus
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.OrbitButton
import dev.okarin.rustygrade.ui.components.OrbitChip
import dev.okarin.rustygrade.ui.screens.SettingLabel
import dev.okarin.rustygrade.ui.screens.ToggleRow
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import uniffi.rustygrade_ffi.UsbHardwareVolumeDto
import uniffi.rustygrade_ffi.eqPresets
import java.util.Locale
import kotlin.math.abs

/** Audio controls: 10-band EQ and presets, ReplayGain mode and userspace Direct USB DAC driver. */
@Composable
fun AudioSettings() {
    val env = LocalEnv.current
    val settings by env.library.settings.collectAsStateWithLifecycle()
    val usb by PlaybackHub.usb.collectAsStateWithLifecycle()
    val usbVol by PlaybackHub.usbVolume.collectAsStateWithLifecycle()
    val presets = remember { eqPresets() }
    val s = settings ?: return

    Column(verticalArrangement = Arrangement.spacedBy(Space.M)) {
        ToggleRow("EQ 10-BAND", s.eqEnabled, { env.library.setEq(it, s.eqPreset) })
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(Space.Compact)) {
            presets.forEach { preset ->
                OrbitChip(
                    label = preset.name.uppercase(),
                    active = s.eqPreset.equals(preset.name, ignoreCase = true),
                    onClick = { env.library.setEq(true, preset.name) },
                )
            }
        }

        Divider()
        SettingLabel("REPLAYGAIN")
        Row(horizontalArrangement = Arrangement.spacedBy(Space.Compact)) {
            listOf("no" to "OFF", "track" to "TRACK", "album" to "ALBUM").forEach { (mode, label) ->
                OrbitChip(label, active = s.replayGain == mode, onClick = { env.library.setReplayGain(mode) })
            }
        }

        Divider()
        ToggleRow(
            "DIRECT USB DAC",
            s.usbBitPerfect,
            { env.library.setUsbBitPerfect(it) },
            caption = "Bit-perfect UAC1/UAC2 output. ReplayGain via DAC hardware.",
        )
        Text(
            usbLabel(usb, usbVol, s.replayGain),
            color = when (usb) {
                is UsbStatus.Active -> Orbit.Teal
                is UsbStatus.Ready, is UsbStatus.PermissionRequired, is UsbStatus.UnsupportedRate -> Orbit.Gold
                is UsbStatus.Failed -> Orbit.Danger
                else -> Orbit.TextDim
            },
            style = OrbitType.Hud,
        )
        if (usb is UsbStatus.PermissionRequired) {
            OrbitButton(
                "GRANT USB ACCESS",
                onClick = { env.playback.requestUsbPermission() },
                primary = true,
            )
        }
        usbNote(usb, usbVol)?.let { Text(it, color = Orbit.TextDim, style = OrbitType.Body) }
    }
}

private fun formatKhz(rateHz: Int): String =
    if (rateHz % 1000 == 0) "${rateHz / 1000} kHz" else "%.1f kHz".format(Locale.US, rateHz / 1000.0)

private fun usbLabel(status: UsbStatus, usbVol: UsbHardwareVolumeDto?, rgMode: String): String = when (status) {
    UsbStatus.Off -> "USB: OFF"
    UsbStatus.NoDevice -> "USB: NO DAC CONNECTED"
    is UsbStatus.PermissionRequired -> "USB: ${status.device} // PERMISSION REQUIRED"
    is UsbStatus.Ready ->
        "USB: ${status.uacVersion} DIRECT READY // ${status.device} // WAITING FOR PLAYBACK"
    is UsbStatus.Active -> {
        val fb = if (status.hasFeedback) "+FB" else ""
        val dspNote = when {
            usbVol == null -> "EQ/REPLAYGAIN BYPASSED"
            abs(usbVol.replayGainDb) > 0.01f ->
                "EQ BYPASSED // RG ${String.format(Locale.US, "%+.1f dB", usbVol.replayGainDb)} HW"
            rgMode != "no" -> "EQ BYPASSED // RG VIA DAC HW"
            else -> "EQ BYPASSED"
        }
        "USB: BIT-PERFECT (${status.uacVersion} ${status.syncMode}$fb) // ${status.device} // " +
            "${formatKhz(status.sampleRate).uppercase()} // ${status.bitResolution}-BIT // $dspNote"
    }
    is UsbStatus.UnsupportedRate ->
        "USB: ${status.device} // ${formatKhz(status.trackRate).uppercase()} NOT SUPPORTED BY DAC // SYSTEM OUTPUT"
    is UsbStatus.Failed -> "USB: FAILED // ${status.reason}"
}

private fun usbNote(status: UsbStatus, usbVol: UsbHardwareVolumeDto?): String? = when (status) {
    is UsbStatus.PermissionRequired -> "Allow USB access to claim the DAC directly."
    is UsbStatus.Ready -> {
        val rates = status.supportedRates.takeIf { it.isNotEmpty() }?.joinToString(", ") { formatKhz(it) }
        rates?.let { "DAC rates: $it." }
    }
    is UsbStatus.Active ->
        if (usbVol != null) {
            "AudioFlinger bypassed. Volume & ReplayGain via DAC hardware."
        } else {
            "AudioFlinger bypassed. Fixed DAC level (ReplayGain off)."
        }
    is UsbStatus.UnsupportedRate -> {
        val rates = status.supportedRates.takeIf { it.isNotEmpty() }?.joinToString(", ") { formatKhz(it) } ?: "unknown"
        "${formatKhz(status.trackRate)} unsupported by DAC ($rates). Using system output."
    }
    else -> null
}

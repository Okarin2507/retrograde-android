package dev.okarin.rustygrade.ui.player

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.database.ContentObserver
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.okarin.rustygrade.media.PlaybackHub
import dev.okarin.rustygrade.media.UsbStatus
import dev.okarin.rustygrade.ui.AudioSettings
import dev.okarin.rustygrade.ui.LocalEnv
import dev.okarin.rustygrade.ui.components.Divider
import dev.okarin.rustygrade.ui.components.Kicker
import dev.okarin.rustygrade.ui.components.OrbitSlider
import dev.okarin.rustygrade.ui.icons.OrbitIcons
import dev.okarin.rustygrade.ui.theme.Dimens
import dev.okarin.rustygrade.ui.theme.LocalAccent
import dev.okarin.rustygrade.ui.theme.Orbit
import dev.okarin.rustygrade.ui.theme.OrbitType
import dev.okarin.rustygrade.ui.theme.Space
import dev.okarin.rustygrade.ui.theme.rememberReadableAccentOn
import java.util.Locale
import kotlin.math.roundToInt

/**
 * Bottom sheet opened from the player: DAC hardware volume (when Direct USB owns the DAC) or
 * system media volume on top, with EQ / ReplayGain / Direct USB controls ([AudioSettings]) below.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun VolumeEqSheet(onDismiss: () -> Unit) {
    val env = LocalEnv.current
    val context = LocalContext.current
    val usb by PlaybackHub.usb.collectAsStateWithLifecycle()
    val usbVol by PlaybackHub.usbVolume.collectAsStateWithLifecycle()
    val directOwned = usb is UsbStatus.Active || usb is UsbStatus.Ready

    val audio = remember(context) { context.getSystemService(Context.AUDIO_SERVICE) as AudioManager }
    val max = remember(audio) { audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).coerceAtLeast(1) }
    var volume by remember { mutableIntStateOf(audio.getStreamVolume(AudioManager.STREAM_MUSIC)) }
    var hwSlider by remember(usbVol?.normalized) { mutableFloatStateOf(usbVol?.normalized ?: 0.5f) }

    // Re-read when the sheet opens, and follow the hardware volume keys while it is showing.
    LaunchedEffect(audio) { volume = audio.getStreamVolume(AudioManager.STREAM_MUSIC) }
    DisposableEffect(context, audio) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                volume = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
            }
        }
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                volume = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
            }
        }
        context.contentResolver.registerContentObserver(Settings.System.CONTENT_URI, true, observer)
        ContextCompat.registerReceiver(
            context,
            receiver,
            IntentFilter("android.media.VOLUME_CHANGED_ACTION"),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        onDispose {
            context.contentResolver.unregisterContentObserver(observer)
            context.unregisterReceiver(receiver)
        }
    }

    val accentTint = rememberReadableAccentOn(Orbit.PanelDeep, LocalAccent.current)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = RectangleShape,
        containerColor = Orbit.PanelDeep,
        contentColor = Orbit.Text,
        dragHandle = { BottomSheetDefaults.DragHandle(color = Orbit.Line) },
    ) {
        Column(
            Modifier
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(horizontal = Space.Gutter)
                .padding(bottom = Space.Xl),
            verticalArrangement = Arrangement.spacedBy(Space.M),
        ) {
            if (directOwned) {
                val hw = usbVol
                if (hw != null) {
                    Kicker("DAC HARDWARE VOLUME")
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                        Icon(
                            if (hwSlider <= 0.001f) OrbitIcons.Volumex else OrbitIcons.Volume2,
                            contentDescription = "DAC hardware volume",
                            tint = Orbit.Teal,
                            modifier = Modifier.size(Dimens.IconM),
                        )
                        OrbitSlider(
                            value = hwSlider,
                            valueRange = 0f..1f,
                            contentDescription = "DAC hardware volume",
                            onValueChange = { v ->
                                hwSlider = v
                                env.playback.setUsbVolume(v)
                            },
                            modifier = Modifier.weight(1f),
                            accent = Orbit.Teal,
                        )
                        Text(
                            String.format(Locale.US, "%.1f dB", hw.userDb),
                            modifier = Modifier.widthIn(min = 58.dp),
                            color = Orbit.Teal,
                            style = OrbitType.NumericBold,
                            textAlign = TextAlign.End,
                        )
                    }
                    if (kotlin.math.abs(hw.replayGainDb) > 0.01f) {
                        Text(
                            String.format(
                                Locale.US,
                                "HW REPLAYGAIN %+.1f dB -> EFFECTIVE %.1f dB // PCM BIT-PERFECT",
                                hw.replayGainDb,
                                hw.currentDb,
                            ),
                            color = Orbit.Gold,
                            style = OrbitType.Hud,
                        )
                    }
                    Text(
                        "Direct DAC hardware volume; PCM stays bit-perfect.",
                        color = Orbit.TextDim,
                        style = OrbitType.Body,
                    )
                } else {
                    Kicker("DAC OUTPUT // FIXED LEVEL")
                    Text(
                        "No DAC hardware volume. Adjust level on the DAC.",
                        color = Orbit.TextDim,
                        style = OrbitType.Body,
                    )
                }
            } else {
                Kicker("VOLUME")
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.S)) {
                    Icon(
                        if (volume == 0) OrbitIcons.Volumex else OrbitIcons.Volume2,
                        contentDescription = "Volume",
                        tint = accentTint,
                        modifier = Modifier.size(Dimens.IconM),
                    )
                    OrbitSlider(
                        value = volume.toFloat(),
                        valueRange = 0f..max.toFloat(),
                        contentDescription = "Media volume",
                        onValueChange = { v ->
                            val level = v.roundToInt().coerceIn(0, max)
                            if (level != volume) {
                                volume = level
                                try {
                                    audio.setStreamVolume(AudioManager.STREAM_MUSIC, level, 0)
                                } catch (_: SecurityException) {
                                    // Some devices refuse while Do Not Disturb is on; the slider re-syncs on reopen.
                                }
                            }
                        },
                        modifier = Modifier.weight(1f),
                        accent = accentTint,
                    )
                    Text(
                        "${volume * 100 / max}%",
                        modifier = Modifier.widthIn(min = 40.dp),
                        color = Orbit.Text,
                        style = OrbitType.NumericBold,
                        textAlign = TextAlign.End,
                    )
                }
            }
            Divider()
            Spacer(Modifier.height(Space.Hair))
            Kicker("AUDIO")
            AudioSettings()
        }
    }
}

package dev.okarin.rustygrade.media

import androidx.media3.common.DeviceInfo
import androidx.media3.common.ForwardingSimpleBasePlayer
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import kotlin.math.roundToInt

/**
 * The player the media session exposes to the lockscreen, notification, Bluetooth and Android Auto.
 *
 * It forwards everything to ExoPlayer except what depends on the Rust queue: skip, shuffle and
 * repeat are routed to [PlaybackCoordinator], and the reported shuffle/repeat state and the
 * available skip commands come from Rust (ExoPlayer only knows a three-item window).
 *
 * When Direct USB owns a DAC with UAC hardware volume control, this player exposes
 * [DeviceInfo.PLAYBACK_TYPE_REMOTE] so Android's system volume HUD and physical volume keys
 * (foreground, background, lockscreen and screen-off) adjust the DAC's hardware Feature Unit
 * directly without altering PCM bits.
 */
@UnstableApi
class RustygradePlayer(
    exo: Player,
    private val coordinator: PlaybackCoordinator,
) : ForwardingSimpleBasePlayer(exo) {

    private var preMuteNormalized: Float = 0.5f

    init {
        coordinator.onStateChanged = { invalidateState() }
        // A crossfade swaps the two ExoPlayers: the session keeps talking to this wrapper.
        coordinator.onPlayerSwapped = { setPlayer(it) }
    }

    override fun getState(): State {
        val base = super.getState()
        val s = coordinator.state
        val usbVol = PlaybackHub.usbVolume.value

        val commandsBuilder = base.availableCommands.buildUpon()
        if (s != null) {
            commandsBuilder
                .add(Player.COMMAND_SET_SHUFFLE_MODE)
                .add(Player.COMMAND_SET_REPEAT_MODE)
                .addIf(Player.COMMAND_SEEK_TO_NEXT, s.canNext)
                .addIf(Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM, s.canNext)
                .removeIf(Player.COMMAND_SEEK_TO_NEXT, !s.canNext)
                .removeIf(Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM, !s.canNext)
                .add(Player.COMMAND_SEEK_TO_PREVIOUS) // always: past 3 s it restarts the track
                .addIf(Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM, s.canPrevious)
                .removeIf(Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM, !s.canPrevious)
        }
        if (usbVol != null) {
            commandsBuilder
                .add(Player.COMMAND_GET_DEVICE_VOLUME)
                .add(Player.COMMAND_SET_DEVICE_VOLUME_WITH_FLAGS)
                .add(Player.COMMAND_ADJUST_DEVICE_VOLUME_WITH_FLAGS)
        }

        val builder = base.buildUpon().setAvailableCommands(commandsBuilder.build())
        if (s != null) {
            builder
                .setShuffleModeEnabled(s.shuffle)
                .setRepeatMode(
                    when (s.repeat) {
                        1 -> Player.REPEAT_MODE_ALL
                        2 -> Player.REPEAT_MODE_ONE
                        else -> Player.REPEAT_MODE_OFF
                    },
                )
        }
        if (usbVol != null) {
            val step = (usbVol.normalized * USB_VOLUME_STEPS).roundToInt().coerceIn(0, USB_VOLUME_STEPS)
            builder
                .setDeviceInfo(REMOTE_USB_DEVICE_INFO)
                .setDeviceVolume(step)
                .setIsDeviceMuted(step == 0)
        }
        return builder.build()
    }

    override fun handleSetDeviceVolume(deviceVolume: Int, flags: Int): ListenableFuture<*> {
        if (PlaybackHub.usbVolume.value != null) {
            val norm = (deviceVolume.toFloat() / USB_VOLUME_STEPS).coerceIn(0f, 1f)
            if (norm > 0.01f) preMuteNormalized = norm
            return coordinator.setUsbHardwareVolume(norm)
        }
        return super.handleSetDeviceVolume(deviceVolume, flags)
    }

    override fun handleIncreaseDeviceVolume(flags: Int): ListenableFuture<*> {
        if (PlaybackHub.usbVolume.value != null) {
            return coordinator.stepUsbHardwareVolume(1f / USB_VOLUME_STEPS)
        }
        return super.handleIncreaseDeviceVolume(flags)
    }

    override fun handleDecreaseDeviceVolume(flags: Int): ListenableFuture<*> {
        if (PlaybackHub.usbVolume.value != null) {
            return coordinator.stepUsbHardwareVolume(-1f / USB_VOLUME_STEPS)
        }
        return super.handleDecreaseDeviceVolume(flags)
    }

    override fun handleSetDeviceMuted(muted: Boolean, flags: Int): ListenableFuture<*> {
        val cur = PlaybackHub.usbVolume.value
        if (cur != null) {
            if (muted && cur.normalized > 0.01f) preMuteNormalized = cur.normalized
            return coordinator.setUsbHardwareVolume(if (muted) 0f else preMuteNormalized.coerceIn(0.05f, 1f))
        }
        return super.handleSetDeviceMuted(muted, flags)
    }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> {
        val current = player.currentMediaItemIndex
        when (seekCommand) {
            // "Next" always moves on in Rust's queue, even when the window's next item is the
            // current one (repeat one, or a one-item window).
            Player.COMMAND_SEEK_TO_NEXT,
            Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM -> coordinator.skipNext()
            // "Previous" follows the Rust queue too (it cannot be derived from the window index: with
            // repeat one the target index equals the current one). Past 3 s it restarts the track.
            Player.COMMAND_SEEK_TO_PREVIOUS -> coordinator.skipPrevious()
            Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM -> coordinator.stepPrevious()
            Player.COMMAND_SEEK_TO_MEDIA_ITEM -> when {
                mediaItemIndex > current -> coordinator.skipNext()
                mediaItemIndex < current -> coordinator.stepPrevious()
                else -> {
                    coordinator.onUserSeek()
                    return super.handleSeek(mediaItemIndex, positionMs, seekCommand)
                }
            }
            else -> {
                coordinator.onUserSeek()
                return super.handleSeek(mediaItemIndex, positionMs, seekCommand)
            }
        }
        return Futures.immediateVoidFuture()
    }

    override fun handleSetRepeatMode(repeatMode: Int): ListenableFuture<*> {
        coordinator.setRepeat(
            when (repeatMode) {
                Player.REPEAT_MODE_ALL -> 1
                Player.REPEAT_MODE_ONE -> 2
                else -> 0
            },
        )
        return Futures.immediateVoidFuture()
    }

    override fun handleSetShuffleModeEnabled(shuffleModeEnabled: Boolean): ListenableFuture<*> {
        coordinator.setShuffle(shuffleModeEnabled)
        return Futures.immediateVoidFuture()
    }

    private companion object {
        const val USB_VOLUME_STEPS = 32
        val REMOTE_USB_DEVICE_INFO: DeviceInfo = DeviceInfo.Builder(DeviceInfo.PLAYBACK_TYPE_REMOTE)
            .setMinVolume(0)
            .setMaxVolume(USB_VOLUME_STEPS)
            .build()
    }
}

package dev.okarin.rustygrade.media

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbInterface
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.core.content.ContextCompat
import java.util.concurrent.ConcurrentHashMap
import uniffi.rustygrade_ffi.UsbAudioDriver
import uniffi.rustygrade_ffi.UsbCapabilitiesDto
import uniffi.rustygrade_ffi.UsbHardwareVolumeDto
import uniffi.rustygrade_ffi.UsbStreamSpecDto
import uniffi.rustygrade_ffi.usbInspectInterfaces

/** Live status of Rustygrade's userspace UAC1/UAC2 bit-perfect USB audio driver. */
sealed interface UsbStatus {
    /** Direct USB toggle is off. */
    data object Off : UsbStatus

    /** Toggle is on, but no USB Audio Class DAC is attached. */
    data object NoDevice : UsbStatus

    /** A USB DAC is attached, waiting for Android USB Host permission from the user. */
    data class PermissionRequired(val device: String) : UsbStatus

    /** USB DAC is opened and claimed exclusively; waiting for playback to configure the clock. */
    data class Ready(
        val device: String,
        val uacVersion: String,
        val supportedRates: List<Int>,
        val supportedBits: List<Int>,
    ) : UsbStatus

    /**
     * Bit-perfect direct USB streaming is active (AudioFlinger bypassed completely, zero resampling).
     */
    data class Active(
        val device: String,
        val uacVersion: String,
        val sampleRate: Int,
        val bitResolution: Int,
        val subslotBytes: Int,
        val channels: Int,
        val syncMode: String,
        val hasFeedback: Boolean,
        val supportedRates: List<Int>,
    ) : UsbStatus

    /**
     * The connected DAC's hardware clock does not natively support this track's sample rate.
     * Rather than resampling inside the USB driver, playback uses Android's system output.
     */
    data class UnsupportedRate(
        val device: String,
        val trackRate: Int,
        val supportedRates: List<Int>,
    ) : UsbStatus

    data class Failed(val reason: String) : UsbStatus
}

/**
 * Pure helper selecting the best USB Audio playback device from a list of attached USB devices.
 * Prefers devices exposing both AudioControl (subclass 1) and AudioStreaming (subclass 2).
 */
internal fun selectAudioDacDevice(devices: Collection<UsbDevice>): UsbDevice? =
    devices
        .filter { dev -> hasAudioStreamingInterface(dev) }
        .maxByOrNull { dev ->
            val hasAc = (0 until dev.interfaceCount).any { i ->
                val iface = dev.getInterface(i)
                iface.interfaceClass == UsbConstants.USB_CLASS_AUDIO && iface.interfaceSubclass == 1
            }
            val hasOutIso = (0 until dev.interfaceCount).any { i ->
                val iface = dev.getInterface(i)
                iface.interfaceClass == UsbConstants.USB_CLASS_AUDIO &&
                    iface.interfaceSubclass == 2 &&
                    (0 until iface.endpointCount).any { e ->
                        val ep = iface.getEndpoint(e)
                        ep.direction == UsbConstants.USB_DIR_OUT &&
                            ep.type == UsbConstants.USB_ENDPOINT_XFER_ISOC
                    }
            }
            (if (hasOutIso) 4 else 0) + (if (hasAc) 2 else 0) + dev.interfaceCount.coerceAtMost(1)
        }

private fun hasAudioStreamingInterface(device: UsbDevice): Boolean =
    (0 until device.interfaceCount).any { i ->
        val iface = device.getInterface(i)
        iface.interfaceClass == UsbConstants.USB_CLASS_AUDIO &&
            (iface.interfaceSubclass == 2 || iface.interfaceSubclass == 1)
    }

/**
 * Manages exclusive userspace access to a connected USB Audio Class (UAC1/UAC2) DAC via Android's
 * [UsbManager] and our Rust `usbdevfs` isochronous driver ([UsbAudioDriver]).
 *
 * Works on all Android 9+ (API 28+) devices without root, bypassing AudioFlinger and the OS
 * resampler completely so the DAC hardware clock locks to the song's exact native sample rate.
 */
class UsbDacController(private val context: Context) {
    private val usbManager = context.getSystemService(Context.USB_SERVICE) as UsbManager
    private val permissionAction = "${context.packageName}.USB_AUDIO_PERMISSION"
    private val attachHandledAction = attachHandledAction(context)
    private val main = Handler(Looper.getMainLooper())

    /**
     * Until when (elapsedRealtime) a missing permission must not be asked for yet. The attach
     * broadcast reaches us before Android grants a remembered ("Always open") USB permission, so
     * prompting right away would show a dialog the user already answered.
     */
    @Volatile
    private var attachSettleUntil = 0L
    private val settledUsbEvent = Runnable { onUsbEvent?.invoke() }

    private class OpenSession(
        val device: UsbDevice,
        val deviceName: String,
        val connection: UsbDeviceConnection,
        val claimedInterfaces: List<UsbInterface>,
        val driver: UsbAudioDriver,
        val capabilities: UsbCapabilitiesDto,
    ) {
        /** Rates the DAC's ranges were checked against; the answer never changes for a session. */
        val rateSupport = ConcurrentHashMap<Int, Boolean>()
    }

    private val lock = Any()
    private var session: OpenSession? = null
    private var lastStreamSpec: UsbStreamSpecDto? = null
    private var permissionRequestedForDeviceId: Int = -1
    private var receiverRegistered = false

    private var onUsbEvent: (() -> Unit)? = null
    private var onActiveDetached: (() -> Unit)? = null
    private var onAudioAttached: (() -> Unit)? = null
    private var onAudioDetached: (() -> Unit)? = null

    /**
     * Until when (elapsedRealtime) USB audio route changes seen by AudioManager are our own doing:
     * claiming the DAC detaches `snd-usb-audio` (the USB output disappears), releasing it and
     * reconnecting the kernel driver brings it back. Those must not pause playback.
     */
    @Volatile
    private var ownRouteChangeUntil = 0L

    private fun markOwnRouteChange() {
        ownRouteChangeUntil = SystemClock.elapsedRealtime() + OWN_ROUTE_CHANGE_MS
    }

    /**
     * True while a USB output change in AudioManager is caused by this controller rather than the
     * user: a session is open (the DAC is ours; a physical unplug arrives as
     * ACTION_USB_DEVICE_DETACHED instead), or a claim / release / kernel reconnect just happened.
     */
    fun suppressesUsbRouteEvents(): Boolean =
        hasOpenSession() || SystemClock.elapsedRealtime() < ownRouteChangeUntil

    private val usbReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                permissionAction -> {
                    val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                    val dev = usbDeviceExtra(intent)
                    Log.i(TAG, "USB permission result: granted=$granted device=${dev?.productName}")
                    if (!granted) {
                        // Allow the user to re-trigger via the button, don't spin automatically.
                        permissionRequestedForDeviceId = dev?.deviceId ?: -1
                    }
                    onUsbEvent?.invoke()
                }
                UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                    permissionRequestedForDeviceId = -1
                    // A newly plugged USB audio device is a route connect even if we claim it next
                    // (its AudioManager appearance may then fall inside the suppression window).
                    val attached = usbDeviceExtra(intent)
                    if (attached != null && hasAudioStreamingInterface(attached)) onAudioAttached?.invoke()
                    // Give Android time to apply a remembered permission before we look (and maybe ask).
                    attachSettleUntil = SystemClock.elapsedRealtime() + ATTACH_SETTLE_MS
                    main.removeCallbacks(settledUsbEvent)
                    main.postDelayed(settledUsbEvent, ATTACH_SETTLE_MS)
                }
                attachHandledAction -> {
                    // UsbAttachActivity ran: Android has applied its USB permission decision.
                    attachSettleUntil = 0L
                    main.removeCallbacks(settledUsbEvent)
                    onUsbEvent?.invoke()
                }
                UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                    val detached = usbDeviceExtra(intent)
                    val wasOwned = synchronized(lock) {
                        val s = session
                        if (s != null && (detached == null || s.device.deviceId == detached.deviceId)) {
                            closeSessionLocked(reconnectKernel = false)
                            true
                        } else {
                            false
                        }
                    }
                    if (detached != null && detached.deviceId == permissionRequestedForDeviceId) {
                        permissionRequestedForDeviceId = -1
                    }
                    if (wasOwned) {
                        onActiveDetached?.invoke()
                    } else {
                        if (detached == null || hasAudioStreamingInterface(detached)) onAudioDetached?.invoke()
                        onUsbEvent?.invoke()
                    }
                }
            }
        }
    }

    /** Registers USB attach/detach and permission broadcasts. Call once on service startup. */
    /**
     * [onAudioDeviceAttached] / [onAudioDeviceDetached]: a USB audio device was physically plugged
     * in / unplugged (the latter only when it was not our open session: that is [onDetachedWhileActive]).
     */
    fun start(
        onChanged: () -> Unit,
        onDetachedWhileActive: () -> Unit,
        onAudioDeviceAttached: () -> Unit = {},
        onAudioDeviceDetached: () -> Unit = {},
    ) {
        onUsbEvent = onChanged
        onActiveDetached = onDetachedWhileActive
        onAudioAttached = onAudioDeviceAttached
        onAudioDetached = onAudioDeviceDetached
        if (receiverRegistered) return
        val systemFilter = IntentFilter().apply {
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        ContextCompat.registerReceiver(context, usbReceiver, systemFilter, ContextCompat.RECEIVER_EXPORTED)
        ContextCompat.registerReceiver(
            context,
            usbReceiver,
            IntentFilter().apply {
                addAction(permissionAction)
                addAction(attachHandledAction)
            },
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        receiverRegistered = true
    }

    /**
     * Reconciles the USB driver with the user's [enabled] toggle and the current track's
     * [trackSampleRate] (0 if unknown / no track loaded).
     *
     * When [enabled] is true and a USB Audio DAC is connected and permitted, opens the Rust
     * `usbdevfs` session exclusively unless [trackSampleRate] is known to be unsupported by the
     * DAC hardware (in which case the DAC is released back to the OS for this track).
     *
     * [close] closes the open session when a stale device or an unsupported rate needs it gone. The
     * coordinator passes one that first releases a player still streaming through
     * [UsbDirectAudioSink]; it is called from this thread, never while holding [lock].
     */
    fun sync(
        enabled: Boolean,
        trackSampleRate: Int,
        autoRequestPermission: Boolean = true,
        close: (reconnectKernel: Boolean) -> Unit = { closeSession(it) },
    ): UsbStatus {
        if (!enabled) {
            closeSession(reconnectKernel = true)
            permissionRequestedForDeviceId = -1
            return UsbStatus.Off
        }

        val dac = selectAudioDacDevice(usbManager.deviceList.values) ?: run {
            closeSession(reconnectKernel = false)
            permissionRequestedForDeviceId = -1
            return UsbStatus.NoDevice
        }
        val name = dacDisplayName(dac)

        // If another device was open (or the current one disconnected), drop it first. Decided under
        // [lock], closed outside it: [close] may release a player whose audio thread needs [lock].
        val staleReconnect: Boolean? = synchronized(lock) {
            val current = session
            if (current != null && (current.device.deviceId != dac.deviceId || current.driver.isDisconnected())) {
                current.device.deviceId != dac.deviceId
            } else {
                null
            }
        }
        if (staleReconnect != null) close(staleReconnect)

        if (!usbManager.hasPermission(dac)) {
            val settling = SystemClock.elapsedRealtime() < attachSettleUntil
            if (autoRequestPermission && !settling && permissionRequestedForDeviceId != dac.deviceId) {
                permissionRequestedForDeviceId = dac.deviceId
                requestPermission(dac)
            }
            return UsbStatus.PermissionRequired(name)
        }

        val active = synchronized(lock) { session } ?: run {
            when (val opened = openSession(dac, name)) {
                is OpenResult.Ok -> opened.session
                is OpenResult.Err -> return UsbStatus.Failed(opened.reason)
            }
        }

        val caps = active.capabilities
        val supportedRates = caps.supportedRates.map { it.toInt() }
        val supportedBits = caps.supportedBitDepths.map { it.toInt() and 0xFF }

        // If we know the track's sample rate and this DAC hardware genuinely cannot run at it,
        // hand the DAC back to Android so playback can fall back cleanly without resampling inside
        // our USB driver.
        if (trackSampleRate > 0 && !supportsRate(active, trackSampleRate)) {
            Log.w(TAG, "$name does not support $trackSampleRate Hz natively (supported: $supportedRates)")
            close(true)
            return UsbStatus.UnsupportedRate(name, trackSampleRate, supportedRates)
        }

        val spec = synchronized(lock) { lastStreamSpec }
        return if (spec != null && (trackSampleRate <= 0 || spec.sampleRate.toInt() == trackSampleRate)) {
            statusFromSpec(active, spec)
        } else {
            UsbStatus.Ready(name, caps.uacVersion, supportedRates, supportedBits)
        }
    }

    /** Explicitly prompts Android's USB Host permission dialog for the connected DAC. */
    fun requestPermissionNow() {
        val dac = selectAudioDacDevice(usbManager.deviceList.values) ?: return
        permissionRequestedForDeviceId = dac.deviceId
        requestPermission(dac)
    }

    /** True when the Rust USB driver currently owns the DAC and is ready for [UsbDirectAudioSink]. */
    fun hasOpenSession(): Boolean = synchronized(lock) {
        val s = session
        s != null && !s.driver.isDisconnected()
    }

    /** True when the owned DAC exposes a UAC Feature Unit hardware volume control. */
    fun hasHardwareVolume(): Boolean = PlaybackHub.usbVolume.value != null

    /** Active Rust driver for [UsbDirectAudioSink] (null if not open or disconnected). */
    fun activeDriver(): UsbAudioDriver? = synchronized(lock) {
        session?.takeIf { !it.driver.isDisconnected() }?.driver
    }

    /**
     * Whether [s]'s DAC runs [rate] natively. The cached rate list answers the common case without
     * the driver's EP0 control lock (a volume transfer can hold it ~1 s); a rate off that list, which
     * a UAC2 subrange or UAC1 continuous range may still cover, asks the driver once per session.
     */
    private fun supportsRate(s: OpenSession, rate: Int): Boolean {
        if (s.capabilities.supportedRates.any { it.toInt() == rate }) return true
        return s.rateSupport.getOrPut(rate) {
            try {
                s.driver.supportsSampleRate(rate.toUInt())
            } catch (_: Exception) {
                false
            }
        }
    }

    /** Hardware rates of the open DAC from its cached capabilities (empty when none is open). */
    fun supportedRates(): List<Int> = synchronized(lock) {
        session?.capabilities?.supportedRates?.map { it.toInt() }.orEmpty()
    }

    /** Called by [UsbDirectAudioSink] once the DAC hardware clock locks to a new stream spec. */
    fun onStreamConfigured(spec: UsbStreamSpecDto): UsbStatus? = synchronized(lock) {
        lastStreamSpec = spec
        val s = session ?: return null
        statusFromSpec(s, spec)
    }

    /** Adjusts the connected DAC's UAC Feature Unit hardware volume (`0f..1f`), PCM bits untouched. */
    fun setHardwareVolume(normalized: Float): Boolean {
        val target = normalized.coerceIn(0f, 1f)
        // EP0 transfers can take ~1 s on some DACs: never hold [lock] across one, or the audio
        // thread's [activeDriver] lookups would stall behind it. A session closed meanwhile makes the
        // call throw (destroyed or disconnected driver), which is caught below.
        val driver = synchronized(lock) { session?.driver } ?: return false
        val updated: UsbHardwareVolumeDto? = try {
            driver.setHardwareVolume(target)
        } catch (e: Exception) {
            Log.w(TAG, "setHardwareVolume($target) failed: ${e.message}")
            null
        }
        if (updated != null && publishVolume(driver, updated)) {
            Log.i(
                TAG,
                "DAC hardware volume: user=%.1f dB rg=%+.2f dB eff=%.1f dB (%.0f%%)".format(
                    updated.userDb,
                    updated.replayGainDb,
                    updated.currentDb,
                    updated.normalized * 100f,
                ),
            )
            return true
        }
        return false
    }

    /**
     * Applies a per-track ReplayGain offset in dB through the DAC's hardware Feature Unit while
     * keeping USB PCM 100% bit-perfect (`effective = user_db + gainDb`, clamped to hardware range).
     */
    fun setReplayGainDb(gainDb: Float): Boolean {
        // Outside [lock], like [setHardwareVolume].
        val driver = synchronized(lock) { session?.driver } ?: return false
        val updated: UsbHardwareVolumeDto? = try {
            driver.setReplayGainDb(gainDb)
        } catch (e: Exception) {
            Log.w(TAG, "setReplayGainDb($gainDb) failed: ${e.message}")
            null
        }
        if (updated != null && publishVolume(driver, updated)) {
            Log.i(
                TAG,
                "DAC HW ReplayGain: %+.2f dB (user=%.1f dB -> eff=%.1f dB)".format(
                    updated.replayGainDb,
                    updated.userDb,
                    updated.currentDb,
                ),
            )
            return true
        }
        return false
    }

    /** Publishes [v] unless [driver]'s session was closed while the transfer ran (that cleared it). */
    private fun publishVolume(driver: UsbAudioDriver, v: UsbHardwareVolumeDto): Boolean = synchronized(lock) {
        if (session?.driver !== driver) return false
        PlaybackHub.setUsbVolume(v)
        true
    }

    /** Closes any active USB session, reattaching `snd-usb-audio` so system audio works again. */
    fun closeSession(reconnectKernel: Boolean = true) {
        synchronized(lock) {
            closeSessionLocked(reconnectKernel)
        }
    }

    fun release() {
        if (receiverRegistered) {
            receiverRegistered = false
            runCatching { context.unregisterReceiver(usbReceiver) }
        }
        main.removeCallbacks(settledUsbEvent)
        closeSession(reconnectKernel = true)
        onUsbEvent = null
        onActiveDetached = null
        onAudioAttached = null
        onAudioDetached = null
    }

    private sealed interface OpenResult {
        data class Ok(val session: OpenSession) : OpenResult
        data class Err(val reason: String) : OpenResult
    }

    private fun openSession(dac: UsbDevice, name: String): OpenResult {
        markOwnRouteChange()
        try {
            return openSessionInner(dac, name)
        } finally {
            markOwnRouteChange() // the window runs from when the claim finished (or failed)
        }
    }

    private fun openSessionInner(dac: UsbDevice, name: String): OpenResult {
        val conn = usbManager.openDevice(dac)
            ?: return OpenResult.Err("Could not open USB device $name")
        val raw = conn.rawDescriptors
        if (raw == null || raw.size < 18) {
            conn.close()
            return OpenResult.Err("Device $name exposed no USB descriptors")
        }
        val preferredConfig: UByte? = if (dac.configurationCount > 0) {
            dac.getConfiguration(0).id.toUByte()
        } else {
            null
        }

        val requiredIfaces = try {
            usbInspectInterfaces(raw, preferredConfig).map { it.toInt() and 0xFF }
        } catch (e: Exception) {
            conn.close()
            return OpenResult.Err(e.message ?: "Not a playable USB Audio DAC")
        }

        // Claim ONLY the AudioControl and AudioStreaming OUT interfaces (force=true detaches
        // kernel snd-usb-audio cleanly while leaving any HID button interfaces untouched).
        val claimed = mutableListOf<UsbInterface>()
        for (ifNum in requiredIfaces) {
            val iface = findInterfaceAlt0(dac, ifNum)
            if (iface == null) {
                releaseInterfaces(conn, claimed)
                conn.close()
                return OpenResult.Err("Missing USB interface #$ifNum on $name")
            }
            if (!conn.claimInterface(iface, true)) {
                releaseInterfaces(conn, claimed)
                conn.close()
                return OpenResult.Err("Could not claim USB audio interface #$ifNum on $name")
            }
            claimed += iface
        }

        val driver = try {
            UsbAudioDriver.open(conn.fileDescriptor, raw, preferredConfig)
        } catch (e: Exception) {
            releaseInterfaces(conn, claimed)
            conn.close()
            return OpenResult.Err(e.message ?: "Failed to initialize UAC driver on $name")
        }

        val caps = driver.capabilities()
        Log.i(
            TAG,
            "Opened $name (${caps.uacVersion}/${caps.usbSpeed}): rates=${caps.supportedRates} " +
                "bits=${caps.supportedBitDepths.map { it.toInt() and 0xFF }} asyncFb=${caps.hasAsyncFeedback} " +
                "hwVol=${caps.hardwareVolume != null}",
        )
        val opened = OpenSession(dac, name, conn, claimed, driver, caps)
        synchronized(lock) {
            closeSessionLocked(reconnectKernel = true)
            session = opened
            lastStreamSpec = null
            // Inside [lock], like [publishVolume]: a later volume write must not be overwritten.
            PlaybackHub.setUsbVolume(caps.hardwareVolume)
        }
        return OpenResult.Ok(opened)
    }

    private fun closeSessionLocked(reconnectKernel: Boolean) {
        val old = session ?: return
        markOwnRouteChange()
        session = null
        lastStreamSpec = null
        PlaybackHub.setUsbVolume(null)
        runCatching { old.driver.shutdown() }
        releaseInterfaces(old.connection, old.claimedInterfaces)
        if (reconnectKernel) {
            runCatching { old.driver.reconnectKernelDrivers() }
        }
        runCatching { old.driver.destroy() }
        runCatching { old.connection.close() }
        markOwnRouteChange() // the kernel driver's output comes back after this
        Log.i(TAG, "Closed USB DAC session: ${old.deviceName} (reconnectKernel=$reconnectKernel)")
    }

    private fun statusFromSpec(s: OpenSession, spec: UsbStreamSpecDto): UsbStatus.Active =
        UsbStatus.Active(
            device = s.deviceName,
            uacVersion = spec.uacVersion,
            sampleRate = spec.sampleRate.toInt(),
            bitResolution = spec.bitResolution.toInt() and 0xFF,
            subslotBytes = spec.subslotBytes.toInt() and 0xFF,
            channels = spec.channels.toInt() and 0xFF,
            syncMode = spec.syncMode,
            hasFeedback = spec.hasFeedback,
            supportedRates = s.capabilities.supportedRates.map { it.toInt() },
        )

    private fun requestPermission(device: UsbDevice) {
        val intent = Intent(permissionAction).setPackage(context.packageName)
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pi = PendingIntent.getBroadcast(context, 0, intent, flags)
        try {
            usbManager.requestPermission(device, pi)
        } catch (e: Exception) {
            Log.e(TAG, "requestPermission failed", e)
        }
    }

    private fun findInterfaceAlt0(device: UsbDevice, interfaceNumber: Int): UsbInterface? {
        var fallback: UsbInterface? = null
        for (i in 0 until device.interfaceCount) {
            val iface = device.getInterface(i)
            if (iface.id == interfaceNumber) {
                if (iface.alternateSetting == 0) return iface
                if (fallback == null) fallback = iface
            }
        }
        return fallback
    }

    private fun releaseInterfaces(conn: UsbDeviceConnection, interfaces: List<UsbInterface>) {
        for (iface in interfaces) {
            runCatching { conn.releaseInterface(iface) }
        }
    }

    private fun dacDisplayName(device: UsbDevice): String {
        val prod = device.productName?.trim().orEmpty()
        val mfr = device.manufacturerName?.trim().orEmpty()
        return when {
            prod.isNotEmpty() && mfr.isNotEmpty() && !prod.startsWith(mfr, ignoreCase = true) -> "$mfr $prod"
            prod.isNotEmpty() -> prod
            mfr.isNotEmpty() -> "$mfr USB DAC"
            else -> "USB DAC (%04X:%04X)".format(device.vendorId, device.productId)
        }
    }

    @Suppress("DEPRECATION")
    private fun usbDeviceExtra(intent: Intent): UsbDevice? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }

    companion object {
        private const val TAG = "RustyUsb"

        /** Package-local broadcast [UsbAttachActivity] sends once Android has handled a DAC plug-in. */
        fun attachHandledAction(context: Context): String = "${context.packageName}.USB_AUDIO_ATTACH_HANDLED"

        /** How long after a plug-in to wait for Android's remembered USB permission before asking. */
        private const val ATTACH_SETTLE_MS = 1_200L
        /** How long after our own claim/release a USB output change counts as ours. */
        private const val OWN_ROUTE_CHANGE_MS = 5_000L
    }
}

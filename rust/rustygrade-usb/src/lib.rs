//! Userspace UAC1/UAC2 bit-perfect USB Audio driver over Linux `usbdevfs`.
//!
//! Designed for Android (via `UsbManager.openDevice(device).fileDescriptor`) and Linux:
//! bypasses Android's AudioFlinger / resampler completely, switches the USB DAC hardware clock
//! to the track's exact sample rate, packs PCM samples bit-for-bit without dithering or scaling,
//! streams over multi-URB isochronous USB transfers with explicit asynchronous feedback tracking,
//! and exposes the DAC's UAC Feature Unit hardware volume control.

pub mod descriptors;
pub mod format;
pub mod ring;
pub mod stream;
pub mod timing;
pub mod uac;
pub mod usbfs;

use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use descriptors::{IsoSyncType, ParsedUsbAudio, StreamAltSetting, UacVersion, parse_raw_descriptors};
use format::{InputPcmFormat, pack_pcm_to_wire, select_alt_setting};
use stream::{IsoStreamParams, IsoStreamThread, StreamShared};
use timing::UsbSpeed;
use uac::{ProbedClockRoute, VolumeControlInfo};

#[derive(Debug, thiserror::Error)]
pub enum UsbAudioError {
    #[error("invalid USB descriptors: {0}")]
    InvalidDescriptors(String),
    #[error("no USB AudioControl interface found on this device")]
    NoAudioControlInterface,
    #[error("no playable PCM AudioStreaming OUT interface found on this device")]
    NoAudioStreamingInterface,
    #[error("DAC does not support {requested} Hz natively (supported: {supported:?})")]
    UnsupportedSampleRate { requested: u32, supported: Vec<u32> },
    #[error("DAC does not support {channels}-channel PCM playback")]
    UnsupportedChannels { channels: u8 },
    #[error("DAC rejected {requested} Hz clock (hardware stayed at {actual} Hz)")]
    SampleRateRejected { requested: u32, actual: u32 },
    #[error("USB control transfer failed: {0}")]
    ControlFailed(String),
    #[error("USB stream is not configured")]
    NotConfigured,
    #[error("USB DAC was disconnected")]
    Disconnected,
}

/// Capabilities probed from the connected USB DAC before playback starts.
#[derive(Debug, Clone)]
pub struct UsbDeviceCapabilities {
    pub vendor_id: u16,
    pub product_id: u16,
    pub uac_version: UacVersion,
    pub speed: UsbSpeed,
    pub ac_interface: u8,
    pub as_interface: u8,
    /// Interfaces that must be claimed exclusively by Android `UsbDeviceConnection`.
    pub required_interfaces: Vec<u8>,
    /// All hardware sample rates supported natively by this DAC (sorted, deduplicated).
    pub supported_rates: Vec<u32>,
    /// All PCM bit depths supported natively by this DAC (e.g. `[16, 24, 32]`).
    pub supported_bit_depths: Vec<u8>,
    /// Maximum PCM channels supported on the chosen playback interface.
    pub max_channels: u8,
    pub has_async_feedback: bool,
    pub hardware_volume: Option<HardwareVolumeState>,
}

/// Snapshot of the DAC's UAC Feature Unit hardware volume control.
#[derive(Debug, Clone, Copy)]
pub struct HardwareVolumeState {
    pub min_db: f32,
    pub max_db: f32,
    /// User's chosen listening level in dB (before ReplayGain offset).
    pub user_db: f32,
    /// Active per-track ReplayGain offset applied on the DAC hardware mixer (dB).
    pub replay_gain_db: f32,
    /// Effective hardware dB written to the DAC (`user_db + replay_gain_db`, clamped).
    pub current_db: f32,
    /// User's normalized slider position `[0.0, 1.0]`.
    pub normalized: f32,
}

impl From<VolumeControlInfo> for HardwareVolumeState {
    fn from(v: VolumeControlInfo) -> Self {
        Self {
            min_db: v.min_db(),
            max_db: v.max_db(),
            user_db: v.user_db(),
            replay_gain_db: v.replay_gain_db,
            current_db: v.current_db(),
            normalized: v.normalized(),
        }
    }
}

/// Live parameters of the currently configured bit-perfect USB hardware stream.
#[derive(Debug, Clone)]
pub struct ActiveStreamInfo {
    pub uac_version: UacVersion,
    pub sample_rate: u32,
    pub bit_resolution: u8,
    pub subslot_bytes: u8,
    pub channels: u8,
    pub sync_type: IsoSyncType,
    pub has_feedback: bool,
}

struct ConfiguredStream {
    alt: StreamAltSetting,
    sample_rate: u32,
    in_format: InputPcmFormat,
    in_channels: u8,
    frame_bytes: usize,
    shared: Arc<StreamShared>,
    thread: Option<IsoStreamThread>,
    pack_scratch: Vec<u8>,
}

/// EP0 / descriptor half of a USB DAC session: probed capabilities, clock routing and the UAC
/// Feature Unit hardware volume. Every method that issues a (potentially slow, 500–1000 ms)
/// control transfer lives here, so callers can guard it with a lock separate from the PCM path.
pub struct UsbDeviceControl {
    fd: RawFd,
    parsed: ParsedUsbAudio,
    speed: UsbSpeed,
    probed_clocks: Vec<ProbedClockRoute>,
    supported_rates: Vec<u32>,
    volume: Option<VolumeControlInfo>,
}

impl UsbDeviceControl {
    /// Parses descriptors, forbids autosuspend, parks the AS interface in alt 0 and probes the
    /// DAC's clocks and hardware volume on an already-claimed `usbdevfs` file descriptor.
    pub fn open(fd: RawFd, raw_descriptors: &[u8], preferred_config: Option<u8>) -> Result<Self, UsbAudioError> {
        let parsed = parse_raw_descriptors(raw_descriptors, preferred_config)?;
        usbfs::forbid_suspend(fd);

        let speed = usbfs::query_speed(fd).unwrap_or_else(|| infer_speed(&parsed));

        // Park the streaming interface in zero-bandwidth alt 0 cleanly.
        let _ = usbfs::set_interface(fd, parsed.as_interface(), 0);

        let probed_clocks = if parsed.uac_version == UacVersion::Uac2 {
            uac::probe_uac2_clocks(fd, &parsed)
        } else {
            Vec::new()
        };
        let supported_rates = uac::collect_supported_rates(&parsed, &probed_clocks);
        let volume = parsed
            .feature_unit
            .as_ref()
            .and_then(|fu| uac::probe_hardware_volume(fd, parsed.ac_interface, parsed.uac_version, fu));

        Ok(Self {
            fd,
            parsed,
            speed,
            probed_clocks,
            supported_rates,
            volume,
        })
    }

    pub fn capabilities(&self) -> UsbDeviceCapabilities {
        let mut bit_depths: Vec<u8> = self
            .parsed
            .alt_settings
            .iter()
            .map(|a| a.bit_resolution)
            .collect();
        bit_depths.sort_unstable();
        bit_depths.dedup();

        UsbDeviceCapabilities {
            vendor_id: self.parsed.vendor_id,
            product_id: self.parsed.product_id,
            uac_version: self.parsed.uac_version,
            speed: self.speed,
            ac_interface: self.parsed.ac_interface,
            as_interface: self.parsed.as_interface(),
            required_interfaces: self.parsed.required_interfaces(),
            supported_rates: self.supported_rates.clone(),
            supported_bit_depths: bit_depths,
            max_channels: self
                .parsed
                .alt_settings
                .iter()
                .map(|a| a.channels)
                .max()
                .unwrap_or(2),
            has_async_feedback: self
                .parsed
                .alt_settings
                .iter()
                .any(|a| a.feedback_endpoint.is_some()),
            hardware_volume: self.volume.map(HardwareVolumeState::from),
        }
    }

    /// Returns true if this DAC natively supports `sample_rate` with zero resampling.
    pub fn supports_sample_rate(&self, sample_rate: u32) -> bool {
        match self.parsed.uac_version {
            UacVersion::Uac2 => {
                if self.probed_clocks.is_empty() {
                    return false;
                }
                self.probed_clocks.iter().any(|c| c.supports(sample_rate))
            }
            UacVersion::Uac1 => self
                .parsed
                .alt_settings
                .iter()
                .any(|a| a.uac1_rates.as_ref().is_some_and(|r| r.supports(sample_rate))),
        }
    }

    /// Current hardware DAC volume state (if the DAC exposes a UAC Feature Unit).
    pub fn hardware_volume(&self) -> Option<HardwareVolumeState> {
        self.volume.map(HardwareVolumeState::from)
    }

    /// Adjusts the DAC's hardware Feature Unit base volume (`0.0..=1.0`), preserving any active
    /// per-track ReplayGain dB offset and leaving PCM audio samples 100% bit-perfect.
    pub fn set_hardware_volume(&mut self, normalized: f32) -> Result<Option<HardwareVolumeState>, UsbAudioError> {
        let Some(vol) = self.volume.as_mut() else {
            return Ok(None);
        };
        let user_raw = vol.raw_from_normalized(normalized);
        let effective = vol.effective_raw(user_raw, vol.replay_gain_db);
        uac::set_hardware_volume_raw(self.fd, vol, effective)?;
        vol.user_raw = user_raw;
        Ok(Some(HardwareVolumeState::from(*vol)))
    }

    /// Applies a per-track ReplayGain offset (in dB) through the DAC's UAC hardware volume control
    /// while leaving the USB PCM audio stream 100% bit-perfect untouched.
    pub fn set_replay_gain_db(&mut self, gain_db: f32) -> Result<Option<HardwareVolumeState>, UsbAudioError> {
        let Some(vol) = self.volume.as_mut() else {
            return Ok(None);
        };
        let sanitized = if gain_db.is_finite() { gain_db.clamp(-40.0, 24.0) } else { 0.0 };
        let effective = vol.effective_raw(vol.user_raw, sanitized);
        if effective != vol.cur_raw {
            uac::set_hardware_volume_raw(self.fd, vol, effective)?;
        }
        vol.replay_gain_db = sanitized;
        Ok(Some(HardwareVolumeState::from(*vol)))
    }

    /// Asks the Linux kernel to re-attach `snd-usb-audio` to the AC & AS interfaces.
    /// Call this AFTER Kotlin calls `UsbDeviceConnection.releaseInterface(...)` and BEFORE
    /// `UsbDeviceConnection.close()`.
    pub fn reconnect_kernel_drivers(&self) {
        for iface in self.parsed.required_interfaces() {
            usbfs::reconnect_kernel_driver(self.fd, iface);
        }
    }

    /// Parks the AS interface in zero-bandwidth alt 0 and re-allows USB autosuspend. The stream
    /// thread must already be stopped.
    pub fn park(&self) {
        let _ = usbfs::set_interface(self.fd, self.parsed.as_interface(), 0);
        usbfs::allow_suspend(self.fd);
    }
}

/// Lock-free telemetry handle of one configured stream (an `Arc` of the shared atomics plus the
/// immutable stream geometry). Cheap to clone; never performs a syscall.
#[derive(Clone)]
pub struct StreamTelemetry {
    shared: Arc<StreamShared>,
    sample_rate: u32,
    frame_bytes: usize,
}

impl StreamTelemetry {
    pub fn is_drained(&self) -> bool {
        self.shared.drained.load(Ordering::Acquire)
    }

    pub fn has_pending_data(&self) -> bool {
        self.shared.ring.available_read() > 0 || !self.shared.drained.load(Ordering::Acquire)
    }

    /// Exact playback position in microseconds driven by the USB DAC's completed URB frame counter.
    pub fn played_position_us(&self) -> u64 {
        let frames = self.shared.played_frames.load(Ordering::Acquire);
        (frames.saturating_mul(1_000_000)) / u64::from(self.sample_rate.max(1))
    }

    /// Configured ring buffer duration in microseconds (reported to ExoPlayer).
    pub fn buffer_duration_us(&self) -> u64 {
        let frames = (self.shared.ring.capacity() / self.frame_bytes.max(1)) as u64;
        (frames.saturating_mul(1_000_000)) / u64::from(self.sample_rate.max(1))
    }

    pub fn underrun_count(&self) -> u64 {
        self.shared.ring.underruns()
    }

    pub fn is_disconnected(&self) -> bool {
        self.shared.disconnected.load(Ordering::Acquire)
    }
}

/// PCM half of a USB DAC session: the configured stream, its ring buffer and the real-time
/// isochronous thread. `write_pcm` / `mark_end_of_stream` never issue EP0 control transfers.
pub struct UsbStreamControl {
    fd: RawFd,
    speed: UsbSpeed,
    stream: Option<ConfiguredStream>,
}

impl UsbStreamControl {
    pub fn new(control: &UsbDeviceControl) -> Self {
        Self {
            fd: control.fd,
            speed: control.speed,
            stream: None,
        }
    }

    /// Telemetry handle of the currently configured stream (`None` when unconfigured).
    pub fn telemetry(&self) -> Option<StreamTelemetry> {
        self.stream.as_ref().map(|s| StreamTelemetry {
            shared: Arc::clone(&s.shared),
            sample_rate: s.sample_rate,
            frame_bytes: s.frame_bytes,
        })
    }

    /// Configures the DAC hardware for exact bit-perfect playback at `(sample_rate, channels,
    /// in_format, source_bits)`. Never resamples.
    ///
    /// If the DAC is already running with the exact same hardware parameters (gapless track
    /// transition at the same rate/bit-depth), the stream stays open without glitches.
    pub fn configure(
        &mut self,
        control: &UsbDeviceControl,
        sample_rate: u32,
        in_channels: u8,
        in_format: InputPcmFormat,
        source_bits: u8,
    ) -> Result<ActiveStreamInfo, UsbAudioError> {
        if !control.supports_sample_rate(sample_rate) {
            return Err(UsbAudioError::UnsupportedSampleRate {
                requested: sample_rate,
                supported: control.supported_rates.clone(),
            });
        }
        let wanted_bits = if source_bits > 0 {
            source_bits
        } else {
            in_format.default_bits()
        };
        let alt = select_alt_setting(
            &control.parsed.alt_settings,
            control.parsed.uac_version,
            sample_rate,
            in_channels,
            wanted_bits,
        )
        .ok_or(UsbAudioError::UnsupportedChannels { channels: in_channels })?
        .clone();

        // Fast path: gapless track change with identical USB hardware configuration.
        if let Some(existing) = self.stream.as_mut()
            && !existing.shared.disconnected.load(Ordering::Acquire)
            && existing.sample_rate == sample_rate
            && existing.alt.alt_setting == alt.alt_setting
            && existing.in_channels == in_channels
            && existing.in_format == in_format
        {
            existing.shared.eos.store(false, Ordering::Release);
            existing.shared.drained.store(false, Ordering::Release);
            return Ok(active_info_of(&alt, control.parsed.uac_version, sample_rate));
        }

        // Teardown any existing stream before switching sample rate / altsetting.
        self.stop();

        let confirmed_rate = uac::configure_hardware_stream(
            control.fd,
            &control.parsed,
            &control.probed_clocks,
            &alt,
            sample_rate,
        )?;

        let frame_bytes = usize::from(alt.channels) * usize::from(alt.subslot_bytes);
        // ~350 ms ring buffer (rounded to power of two) — resilient against Android screen-off jitter.
        let target_ring_bytes = (usize::try_from(confirmed_rate).unwrap_or(48_000) * frame_bytes * 350) / 1000;
        let shared = Arc::new(StreamShared::new(target_ring_bytes.max(32_768)));

        self.stream = Some(ConfiguredStream {
            alt: alt.clone(),
            sample_rate: confirmed_rate,
            in_format,
            in_channels,
            frame_bytes,
            shared,
            thread: None,
            pack_scratch: Vec::with_capacity(16_384),
        });

        Ok(active_info_of(&alt, control.parsed.uac_version, confirmed_rate))
    }

    fn stream_params(fd: RawFd, speed: UsbSpeed, s: &ConfiguredStream) -> IsoStreamParams {
        IsoStreamParams {
            fd,
            sample_rate: s.sample_rate,
            frame_bytes: s.frame_bytes,
            speed,
            out_ep: s.alt.out_endpoint.clone(),
            fb_ep: s.alt.feedback_endpoint,
        }
    }

    /// Starts (or resumes) the real-time isochronous streaming thread.
    pub fn start(&mut self) -> Result<(), UsbAudioError> {
        let s = self.stream.as_mut().ok_or(UsbAudioError::NotConfigured)?;
        if s.shared.disconnected.load(Ordering::Acquire) {
            return Err(UsbAudioError::Disconnected);
        }
        if s.thread.is_some() {
            return Ok(());
        }
        let params = Self::stream_params(self.fd, self.speed, s);
        s.thread = Some(IsoStreamThread::start(params, Arc::clone(&s.shared)));
        Ok(())
    }

    /// Pauses the isochronous streaming thread while preserving buffered audio & frame clock.
    pub fn pause(&mut self) {
        if let Some(s) = self.stream.as_mut()
            && let Some(thread) = s.thread.take()
        {
            thread.stop();
        }
    }

    /// Flushes all buffered audio and resets the frame clock (called on seek / track reload).
    pub fn flush(&mut self) {
        if let Some(s) = self.stream.as_mut() {
            let was_running = s.thread.is_some();
            if let Some(thread) = s.thread.take() {
                thread.stop();
            }
            s.shared.ring.clear();
            s.shared.played_frames.store(0, Ordering::Release);
            s.shared.submitted_real_frames.store(0, Ordering::Release);
            s.shared.eos.store(false, Ordering::Release);
            s.shared.drained.store(false, Ordering::Release);
            if was_running && !s.shared.disconnected.load(Ordering::Acquire) {
                let params = Self::stream_params(self.fd, self.speed, s);
                s.thread = Some(IsoStreamThread::start(params, Arc::clone(&s.shared)));
            }
        }
    }

    /// Writes raw PCM bytes (matching the configured `InputPcmFormat`) into the ring buffer.
    ///
    /// Returns the number of **input** bytes consumed (always a whole number of audio frames;
    /// returns `0` when the ring buffer is full so ExoPlayer retries on its next render pass).
    pub fn write_pcm(&mut self, pcm: &[u8]) -> Result<usize, UsbAudioError> {
        let s = self.stream.as_mut().ok_or(UsbAudioError::NotConfigured)?;
        if s.shared.disconnected.load(Ordering::Acquire) {
            return Err(UsbAudioError::Disconnected);
        }
        let in_frame_bytes = usize::from(s.in_channels) * s.in_format.bytes_per_sample();
        if in_frame_bytes == 0 || pcm.len() < in_frame_bytes {
            return Ok(0);
        }
        let free_wire_bytes = s.shared.ring.available_write();
        let max_frames = free_wire_bytes / s.frame_bytes.max(1);
        if max_frames == 0 {
            return Ok(0);
        }
        // Only whole input frames are ever consumed; a trailing partial frame stays with the caller.
        let avail_in_frames = pcm.len() / in_frame_bytes;
        let take_frames = avail_in_frames.min(max_frames);
        let take_in_bytes = take_frames * in_frame_bytes;

        // Zero-copy fast path: the wire layout equals the input layout, so write straight into the
        // ring (the single producer already checked that `take_frames` fit).
        if s.in_channels == s.alt.channels
            && matches!(
                (s.in_format, s.alt.subslot_bytes),
                (InputPcmFormat::Pcm16Le, 2) | (InputPcmFormat::Pcm24Le, 3) | (InputPcmFormat::Pcm32Le, 4)
            )
        {
            let written = s.shared.ring.write(&pcm[..take_in_bytes]);
            return Ok((written / in_frame_bytes) * in_frame_bytes);
        }

        s.pack_scratch.clear();
        let packed_frames = pack_pcm_to_wire(
            &pcm[..take_in_bytes],
            s.in_format,
            s.in_channels,
            s.alt.channels,
            s.alt.subslot_bytes,
            &mut s.pack_scratch,
        );
        if packed_frames == 0 {
            return Ok(0);
        }
        let written_wire = s.shared.ring.write(&s.pack_scratch);
        let written_frames = written_wire / s.frame_bytes;
        Ok(written_frames * in_frame_bytes)
    }

    /// Signals end-of-stream so the streaming thread drains all queued PCM cleanly.
    pub fn mark_end_of_stream(&mut self) {
        if let Some(s) = self.stream.as_mut() {
            s.shared.eos.store(true, Ordering::Release);
            if s.thread.is_none() && s.shared.ring.available_read() == 0 {
                s.shared.drained.store(true, Ordering::Release);
            }
        }
    }

    /// Stops the streaming thread (joining it) and drops the configured stream.
    pub fn stop(&mut self) {
        if let Some(mut s) = self.stream.take()
            && let Some(thread) = s.thread.take()
        {
            thread.stop();
        }
    }
}

/// Live session owning one opened USB DAC file descriptor (the caller keeps `UsbDeviceConnection`
/// alive for the entire lifetime of this struct and closes it only after `close()` / `Drop`).
///
/// Single-owner convenience composite of [`UsbDeviceControl`] + [`UsbStreamControl`]; callers that
/// need the PCM path to stay unblocked by EP0 transfers hold the two halves behind separate locks.
pub struct UsbAudioSession {
    control: UsbDeviceControl,
    stream: UsbStreamControl,
    closed: bool,
}

impl UsbAudioSession {
    /// Parses the DAC's descriptors without touching `fd` (used by Android to learn which
    /// interfaces to `claimInterface(iface, true)` before calling [`Self::open`]).
    pub fn inspect_interfaces(raw_descriptors: &[u8], preferred_config: Option<u8>) -> Result<Vec<u8>, UsbAudioError> {
        let parsed = parse_raw_descriptors(raw_descriptors, preferred_config)?;
        Ok(parsed.required_interfaces())
    }

    /// Opens a bit-perfect USB audio session on an already-claimed `usbdevfs` file descriptor.
    pub fn open(
        fd: RawFd,
        raw_descriptors: &[u8],
        preferred_config: Option<u8>,
    ) -> Result<Self, UsbAudioError> {
        let control = UsbDeviceControl::open(fd, raw_descriptors, preferred_config)?;
        let stream = UsbStreamControl::new(&control);
        Ok(Self {
            control,
            stream,
            closed: false,
        })
    }

    pub fn capabilities(&self) -> UsbDeviceCapabilities {
        self.control.capabilities()
    }

    /// Returns true if this DAC natively supports `sample_rate` with zero resampling.
    pub fn supports_sample_rate(&self, sample_rate: u32) -> bool {
        self.control.supports_sample_rate(sample_rate)
    }

    /// See [`UsbStreamControl::configure`].
    pub fn configure(
        &mut self,
        sample_rate: u32,
        in_channels: u8,
        in_format: InputPcmFormat,
        source_bits: u8,
    ) -> Result<ActiveStreamInfo, UsbAudioError> {
        if self.closed {
            return Err(UsbAudioError::Disconnected);
        }
        self.stream
            .configure(&self.control, sample_rate, in_channels, in_format, source_bits)
    }

    /// Starts (or resumes) the real-time isochronous streaming thread.
    pub fn start(&mut self) -> Result<(), UsbAudioError> {
        if self.closed {
            return Err(UsbAudioError::Disconnected);
        }
        self.stream.start()
    }

    /// Pauses the isochronous streaming thread while preserving buffered audio & frame clock.
    pub fn pause(&mut self) {
        self.stream.pause();
    }

    /// Flushes all buffered audio and resets the frame clock (called on seek / track reload).
    pub fn flush(&mut self) {
        self.stream.flush();
    }

    /// See [`UsbStreamControl::write_pcm`].
    pub fn write_pcm(&mut self, pcm: &[u8]) -> Result<usize, UsbAudioError> {
        if self.closed {
            return Err(UsbAudioError::Disconnected);
        }
        self.stream.write_pcm(pcm)
    }

    /// Signals end-of-stream so the streaming thread drains all queued PCM cleanly.
    pub fn mark_end_of_stream(&mut self) {
        self.stream.mark_end_of_stream();
    }

    pub fn is_drained(&self) -> bool {
        self.stream.telemetry().is_none_or(|t| t.is_drained())
    }

    pub fn has_pending_data(&self) -> bool {
        self.stream.telemetry().is_some_and(|t| t.has_pending_data())
    }

    /// Exact playback position in microseconds driven by the USB DAC's completed URB frame counter.
    pub fn played_position_us(&self) -> u64 {
        self.stream.telemetry().map_or(0, |t| t.played_position_us())
    }

    /// Configured ring buffer duration in microseconds (reported to ExoPlayer).
    pub fn buffer_duration_us(&self) -> u64 {
        self.stream.telemetry().map_or(0, |t| t.buffer_duration_us())
    }

    pub fn underrun_count(&self) -> u64 {
        self.stream.telemetry().map_or(0, |t| t.underrun_count())
    }

    pub fn is_disconnected(&self) -> bool {
        self.closed || self.stream.telemetry().is_some_and(|t| t.is_disconnected())
    }

    /// Current hardware DAC volume state (if the DAC exposes a UAC Feature Unit).
    pub fn hardware_volume(&self) -> Option<HardwareVolumeState> {
        self.control.hardware_volume()
    }

    /// See [`UsbDeviceControl::set_hardware_volume`].
    pub fn set_hardware_volume(&mut self, normalized: f32) -> Result<Option<HardwareVolumeState>, UsbAudioError> {
        if self.closed {
            return Err(UsbAudioError::Disconnected);
        }
        self.control.set_hardware_volume(normalized)
    }

    /// See [`UsbDeviceControl::set_replay_gain_db`].
    pub fn set_replay_gain_db(&mut self, gain_db: f32) -> Result<Option<HardwareVolumeState>, UsbAudioError> {
        if self.closed {
            return Err(UsbAudioError::Disconnected);
        }
        self.control.set_replay_gain_db(gain_db)
    }

    /// Stops streaming, parks AS altsetting 0, re-allows USB autosuspend, and marks the session closed.
    /// Does NOT close `fd` (Android's `UsbDeviceConnection` owns the fd).
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.stream.stop();
        self.control.park();
        self.closed = true;
    }

    /// Asks the Linux kernel to re-attach `snd-usb-audio` to the AC & AS interfaces.
    pub fn reconnect_kernel_drivers(&self) {
        self.control.reconnect_kernel_drivers();
    }
}

impl Drop for UsbAudioSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn active_info_of(alt: &StreamAltSetting, uac_version: UacVersion, sample_rate: u32) -> ActiveStreamInfo {
    ActiveStreamInfo {
        uac_version,
        sample_rate,
        bit_resolution: alt.bit_resolution,
        subslot_bytes: alt.subslot_bytes,
        channels: alt.channels,
        sync_type: alt.out_endpoint.sync_type,
        has_feedback: alt.feedback_endpoint.is_some(),
    }
}

fn infer_speed(parsed: &ParsedUsbAudio) -> UsbSpeed {
    if parsed.bcd_usb >= 0x0300 {
        return UsbSpeed::Super;
    }
    if parsed.uac_version == UacVersion::Uac2
        || parsed.bcd_usb >= 0x0200
            && parsed
                .alt_settings
                .iter()
                .any(|a| a.out_endpoint.max_packet_size > 1023)
    {
        UsbSpeed::High
    } else {
        UsbSpeed::Full
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use descriptors::{IsoEndpoint, IsoSyncType};

    fn stream_with(in_format: InputPcmFormat, in_channels: u8, channels: u8, subslot_bytes: u8) -> UsbStreamControl {
        let alt = StreamAltSetting {
            interface_number: 1,
            alt_setting: 1,
            terminal_link: 1,
            channels,
            subslot_bytes,
            bit_resolution: subslot_bytes * 8,
            out_endpoint: IsoEndpoint {
                address: 0x01,
                sync_type: IsoSyncType::Asynchronous,
                max_packet_size: 1024,
                interval: 1,
                synch_address: 0,
                has_sampling_freq_control: false,
            },
            feedback_endpoint: None,
            uac1_rates: None,
        };
        let frame_bytes = usize::from(channels) * usize::from(subslot_bytes);
        UsbStreamControl {
            fd: -1,
            speed: UsbSpeed::High,
            stream: Some(ConfiguredStream {
                alt,
                sample_rate: 48_000,
                in_format,
                in_channels,
                frame_bytes,
                shared: Arc::new(StreamShared::new(4096)),
                thread: None,
                pack_scratch: Vec::new(),
            }),
        }
    }

    fn drain(s: &UsbStreamControl) -> Vec<u8> {
        let shared = &s.stream.as_ref().unwrap().shared;
        let mut out = vec![0u8; shared.ring.available_read()];
        shared.ring.read_or_silence(&mut out, false);
        out
    }

    #[test]
    fn passthrough_writes_whole_frames_straight_into_the_ring() {
        let mut s = stream_with(InputPcmFormat::Pcm24Le, 2, 2, 3);
        // 2 whole 6-byte frames plus a 4-byte partial frame: only the whole frames are consumed.
        let pcm: Vec<u8> = (0..16).collect();
        assert_eq!(s.write_pcm(&pcm).unwrap(), 12);
        assert_eq!(drain(&s), pcm[..12]);
        assert!(s.stream.as_ref().unwrap().pack_scratch.is_empty(), "fast path skips the scratch buffer");
    }

    #[test]
    fn passthrough_stops_at_ring_capacity_on_a_frame_boundary() {
        let mut s = stream_with(InputPcmFormat::Pcm16Le, 2, 2, 2);
        let pcm = vec![7u8; 10_000];
        let written = s.write_pcm(&pcm).unwrap();
        assert_eq!(written, 4096, "ring capacity, a whole number of 4-byte frames");
        assert_eq!(s.write_pcm(&pcm).unwrap(), 0, "full ring: retry later");
    }

    #[test]
    fn converting_formats_still_pack_and_report_input_bytes() {
        // 16-bit input onto a 24-bit-in-32 (4-byte subslot) wire: packed through the scratch buffer.
        let mut s = stream_with(InputPcmFormat::Pcm16Le, 2, 2, 4);
        let pcm = [0x34, 0x12, 0x78, 0x56, 0xFF];
        assert_eq!(s.write_pcm(&pcm).unwrap(), 4);
        assert_eq!(drain(&s), [0, 0, 0x34, 0x12, 0, 0, 0x78, 0x56]);
    }

    #[test]
    fn telemetry_reads_the_stream_atomics() {
        let s = stream_with(InputPcmFormat::Pcm16Le, 2, 2, 2);
        let t = s.telemetry().expect("configured");
        assert!(!t.is_drained() && t.has_pending_data() && !t.is_disconnected());
        s.stream.as_ref().unwrap().shared.played_frames.store(48_000, Ordering::Release);
        assert_eq!(t.played_position_us(), 1_000_000);
        assert_eq!(t.buffer_duration_us(), (4096 / 4) * 1_000_000 / 48_000);
        assert!(UsbStreamControl { fd: -1, speed: UsbSpeed::High, stream: None }.telemetry().is_none());
    }
}

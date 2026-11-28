//! UniFFI bridge for [`rustygrade_usb::UsbAudioSession`].
//!
//! Android opens the USB DAC via `UsbManager.openDevice()`, claims the AC + AS interfaces
//! returned by [`usb_inspect_interfaces`], and hands the `usbdevfs` file descriptor + raw
//! descriptors to [`UsbAudioDriver::open`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use rustygrade_usb::descriptors::UacVersion;
use rustygrade_usb::format::InputPcmFormat;
use rustygrade_usb::timing::UsbSpeed;
use rustygrade_usb::{
    ActiveStreamInfo, HardwareVolumeState, StreamTelemetry, UsbAudioError, UsbAudioSession, UsbDeviceCapabilities,
    UsbDeviceControl, UsbStreamControl,
};

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum UsbDriverError {
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    UnsupportedRate(String),
    #[error("{0}")]
    Disconnected(String),
}

impl From<UsbAudioError> for UsbDriverError {
    fn from(e: UsbAudioError) -> Self {
        match e {
            UsbAudioError::UnsupportedSampleRate { .. } | UsbAudioError::SampleRateRejected { .. } => {
                Self::UnsupportedRate(e.to_string())
            }
            UsbAudioError::Disconnected => Self::Disconnected(e.to_string()),
            other => Self::Failed(other.to_string()),
        }
    }
}

/// PCM sample layout delivered from Media3 / ExoPlayer into the USB driver.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum UsbPcmEncoding {
    Pcm16Le,
    Pcm24Le,
    Pcm24Be,
    Pcm32Le,
    Pcm32Be,
    PcmFloat32Le,
}

impl From<UsbPcmEncoding> for InputPcmFormat {
    fn from(e: UsbPcmEncoding) -> Self {
        match e {
            UsbPcmEncoding::Pcm16Le => Self::Pcm16Le,
            UsbPcmEncoding::Pcm24Le => Self::Pcm24Le,
            UsbPcmEncoding::Pcm24Be => Self::Pcm24Be,
            UsbPcmEncoding::Pcm32Le => Self::Pcm32Le,
            UsbPcmEncoding::Pcm32Be => Self::Pcm32Be,
            UsbPcmEncoding::PcmFloat32Le => Self::PcmFloat32Le,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct UsbHardwareVolumeDto {
    pub min_db: f32,
    pub max_db: f32,
    pub user_db: f32,
    pub replay_gain_db: f32,
    pub current_db: f32,
    pub normalized: f32,
}

impl From<HardwareVolumeState> for UsbHardwareVolumeDto {
    fn from(v: HardwareVolumeState) -> Self {
        Self {
            min_db: v.min_db,
            max_db: v.max_db,
            user_db: v.user_db,
            replay_gain_db: v.replay_gain_db,
            current_db: v.current_db,
            normalized: v.normalized,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct UsbCapabilitiesDto {
    pub vendor_id: u16,
    pub product_id: u16,
    /// `"UAC1"` or `"UAC2"`.
    pub uac_version: String,
    /// `"FS"`, `"HS"`, or `"SS"`.
    pub usb_speed: String,
    pub ac_interface: u8,
    pub as_interface: u8,
    pub required_interfaces: Vec<u8>,
    pub supported_rates: Vec<u32>,
    pub supported_bit_depths: Vec<u8>,
    pub max_channels: u8,
    pub has_async_feedback: bool,
    pub hardware_volume: Option<UsbHardwareVolumeDto>,
}

impl From<UsbDeviceCapabilities> for UsbCapabilitiesDto {
    fn from(c: UsbDeviceCapabilities) -> Self {
        Self {
            vendor_id: c.vendor_id,
            product_id: c.product_id,
            uac_version: match c.uac_version {
                UacVersion::Uac1 => "UAC1".to_owned(),
                UacVersion::Uac2 => "UAC2".to_owned(),
            },
            usb_speed: match c.speed {
                UsbSpeed::Full => "FS".to_owned(),
                UsbSpeed::High => "HS".to_owned(),
                UsbSpeed::Super => "SS".to_owned(),
            },
            ac_interface: c.ac_interface,
            as_interface: c.as_interface,
            required_interfaces: c.required_interfaces,
            supported_rates: c.supported_rates,
            supported_bit_depths: c.supported_bit_depths,
            max_channels: c.max_channels,
            has_async_feedback: c.has_async_feedback,
            hardware_volume: c.hardware_volume.map(UsbHardwareVolumeDto::from),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct UsbStreamSpecDto {
    pub uac_version: String,
    pub sample_rate: u32,
    pub bit_resolution: u8,
    pub subslot_bytes: u8,
    pub channels: u8,
    /// `"ASYNC"`, `"ADAPTIVE"`, `"SYNC"`, or `"ISO"`.
    pub sync_mode: String,
    pub has_feedback: bool,
}

impl From<ActiveStreamInfo> for UsbStreamSpecDto {
    fn from(s: ActiveStreamInfo) -> Self {
        Self {
            uac_version: match s.uac_version {
                UacVersion::Uac1 => "UAC1".to_owned(),
                UacVersion::Uac2 => "UAC2".to_owned(),
            },
            sample_rate: s.sample_rate,
            bit_resolution: s.bit_resolution,
            subslot_bytes: s.subslot_bytes,
            channels: s.channels,
            sync_mode: s.sync_type.label().to_owned(),
            has_feedback: s.has_feedback,
        }
    }
}

/// Parses raw USB descriptors and returns the interface numbers (`AC` + `AS OUT`) that Android
/// should exclusively claim with `UsbDeviceConnection.claimInterface(iface, true)`.
#[uniffi::export]
pub fn usb_inspect_interfaces(
    raw_descriptors: Vec<u8>,
    preferred_config: Option<u8>,
) -> Result<Vec<u8>, UsbDriverError> {
    Ok(UsbAudioSession::inspect_interfaces(&raw_descriptors, preferred_config)?)
}

/// Native bit-perfect UAC1/UAC2 USB audio driver session.
///
/// State is split so slow EP0 control transfers (hardware volume / ReplayGain: 500–1000 ms on some
/// DACs) never block ExoPlayer's audio thread or position polling:
///
/// * `closed` — atomic, checked first by every call.
/// * `control` — EP0 / descriptor state ([`UsbDeviceControl`]).
/// * `stream` — ring buffer + iso thread ([`UsbStreamControl`]); the only lock `write_pcm` takes.
/// * `telemetry` — `Arc` snapshot of the stream atomics, swapped only by `configure` / `shutdown`
///   and never held across a syscall.
///
/// Strict lock order: `control` -> `stream` -> `telemetry`. Every call that can start, stop or
/// reconfigure the stream (or park the device) holds `control` first, so the fd can never be parked
/// while an EP0 transfer or a stream-thread start is in progress.
#[derive(uniffi::Object)]
pub struct UsbAudioDriver {
    closed: AtomicBool,
    control: Mutex<UsbDeviceControl>,
    stream: Mutex<UsbStreamControl>,
    telemetry: Mutex<Option<StreamTelemetry>>,
}

#[uniffi::export]
impl UsbAudioDriver {
    /// Opens the USB DAC on `fd` (from `UsbDeviceConnection.getFileDescriptor()`).
    #[uniffi::constructor]
    pub fn open(
        fd: i32,
        raw_descriptors: Vec<u8>,
        preferred_config: Option<u8>,
    ) -> Result<Self, UsbDriverError> {
        let control = UsbDeviceControl::open(fd, &raw_descriptors, preferred_config)?;
        let stream = UsbStreamControl::new(&control);
        Ok(Self {
            closed: AtomicBool::new(false),
            control: Mutex::new(control),
            stream: Mutex::new(stream),
            telemetry: Mutex::new(None),
        })
    }

    pub fn capabilities(&self) -> UsbCapabilitiesDto {
        self.control().capabilities().into()
    }

    pub fn supports_sample_rate(&self, sample_rate: u32) -> bool {
        self.control().supports_sample_rate(sample_rate)
    }

    pub fn configure(
        &self,
        sample_rate: u32,
        channels: u8,
        pcm_encoding: UsbPcmEncoding,
        source_bits: u8,
    ) -> Result<UsbStreamSpecDto, UsbDriverError> {
        let control = self.control();
        self.ensure_open()?;
        let mut stream = self.stream();
        let result = stream.configure(&control, sample_rate, channels, pcm_encoding.into(), source_bits);
        // Publish whatever stream now exists, on success AND failure: a failed reconfigure has torn
        // the old stream down, and stale telemetry would report a drain that can never complete.
        *self.telemetry() = stream.telemetry();
        Ok(result?.into())
    }

    pub fn start(&self) -> Result<(), UsbDriverError> {
        let _control = self.control();
        self.ensure_open()?;
        self.stream().start()?;
        Ok(())
    }

    pub fn pause(&self) {
        let _control = self.control();
        self.stream().pause();
    }

    pub fn flush(&self) {
        let _control = self.control();
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.stream().flush();
    }

    pub fn write_pcm(&self, pcm: Vec<u8>) -> Result<u32, UsbDriverError> {
        self.ensure_open()?;
        let mut stream = self.stream();
        // Re-check under the lock: a `shutdown` that won the race leaves no stream behind, which
        // must surface as `Disconnected`, not `NotConfigured`.
        self.ensure_open()?;
        let consumed = stream.write_pcm(&pcm)?;
        Ok(consumed as u32)
    }

    pub fn mark_end_of_stream(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.stream().mark_end_of_stream();
    }

    pub fn is_drained(&self) -> bool {
        self.telemetry().as_ref().is_none_or(|t| t.is_drained())
    }

    pub fn has_pending_data(&self) -> bool {
        self.telemetry().as_ref().is_some_and(|t| t.has_pending_data())
    }

    pub fn played_position_us(&self) -> u64 {
        self.telemetry().as_ref().map_or(0, |t| t.played_position_us())
    }

    pub fn buffer_duration_us(&self) -> u64 {
        self.telemetry().as_ref().map_or(0, |t| t.buffer_duration_us())
    }

    pub fn is_disconnected(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.telemetry().as_ref().is_some_and(|t| t.is_disconnected())
    }

    pub fn hardware_volume(&self) -> Option<UsbHardwareVolumeDto> {
        self.control().hardware_volume().map(UsbHardwareVolumeDto::from)
    }

    pub fn set_hardware_volume(&self, normalized: f32) -> Result<Option<UsbHardwareVolumeDto>, UsbDriverError> {
        let mut control = self.control();
        self.ensure_open()?;
        let updated = control.set_hardware_volume(normalized)?;
        Ok(updated.map(UsbHardwareVolumeDto::from))
    }

    pub fn set_replay_gain_db(&self, gain_db: f32) -> Result<Option<UsbHardwareVolumeDto>, UsbDriverError> {
        let mut control = self.control();
        self.ensure_open()?;
        let updated = control.set_replay_gain_db(gain_db)?;
        Ok(updated.map(UsbHardwareVolumeDto::from))
    }

    /// Stops streaming, parks AS altsetting 0, and re-allows USB autosuspend without closing `fd`.
    pub fn shutdown(&self) {
        let control = self.control();
        let mut stream = self.stream();
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // The iso thread takes no locks, so joining it while holding both is deadlock-free.
        stream.stop();
        control.park();
        *self.telemetry() = None;
    }

    pub fn reconnect_kernel_drivers(&self) {
        self.control().reconnect_kernel_drivers();
    }
}

impl UsbAudioDriver {
    fn ensure_open(&self) -> Result<(), UsbAudioError> {
        if self.closed.load(Ordering::Acquire) {
            Err(UsbAudioError::Disconnected)
        } else {
            Ok(())
        }
    }

    fn control(&self) -> MutexGuard<'_, UsbDeviceControl> {
        self.control.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn stream(&self) -> MutexGuard<'_, UsbStreamControl> {
        self.stream.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn telemetry(&self) -> MutexGuard<'_, Option<StreamTelemetry>> {
        self.telemetry.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Drop for UsbAudioDriver {
    /// Dropping the UniFFI object must stop the iso thread before Kotlin closes the fd.
    fn drop(&mut self) {
        self.shutdown();
    }
}

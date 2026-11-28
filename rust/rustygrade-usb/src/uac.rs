//! USB Audio Class 1.0 & 2.0 control-plane operations over `EP0`:
//! - UAC2 Clock Source sample-rate `RANGE` discovery, `SET_CUR`/`GET_CUR` switching, Clock Selector
//!   routing, and `CS_CLOCK_VALID_CONTROL` verification
//! - UAC1 Endpoint `SAMPLING_FREQ_CONTROL` (`SET_CUR`/`GET_CUR`, 3-byte LE)
//! - UAC1 & UAC2 Feature Unit hardware DAC volume (`RANGE`/`MIN`/`MAX`/`RES`/`CUR`) & mute.

use std::os::unix::io::RawFd;

use crate::UsbAudioError;
use crate::descriptors::{ClockRoute, FeatureUnit, ParsedUsbAudio, StreamAltSetting, Uac1SampleRates, UacVersion};
use crate::usbfs;

// USB bmRequestType constants for Audio Class control transfers.
const REQ_CLASS_IF_OUT: u8 = 0x21;
const REQ_CLASS_IF_IN: u8 = 0xA1;
const REQ_CLASS_EP_OUT: u8 = 0x22;
const REQ_CLASS_EP_IN: u8 = 0xA2;

// UAC1 bRequest codes.
const UAC1_SET_CUR: u8 = 0x01;
const UAC1_GET_CUR: u8 = 0x81;
const UAC1_GET_MIN: u8 = 0x82;
const UAC1_GET_MAX: u8 = 0x83;
const UAC1_GET_RES: u8 = 0x84;

// UAC2 bRequest codes.
const UAC2_CUR: u8 = 0x01;
const UAC2_RANGE: u8 = 0x02;

// Control selectors (placed in high byte of wValue).
const CS_SAM_FREQ_CONTROL: u16 = 0x01 << 8;
const CS_CLOCK_VALID_CONTROL: u16 = 0x02 << 8;
const CX_CLOCK_SELECTOR_CONTROL: u16 = 0x01 << 8;
const FU_MUTE_CONTROL: u16 = 0x01 << 8;
const FU_VOLUME_CONTROL: u16 = 0x02 << 8;
const EP_SAMPLING_FREQ_CONTROL: u16 = 0x01 << 8;

const CTRL_TIMEOUT_MS: u32 = 500;

/// Standard audio sample rates probed when a DAC exposes a continuous range.
pub const STANDARD_RATES: &[u32] = &[
    8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    352_800, 384_000, 705_600, 768_000,
];

/// One UAC2 4-byte range triplet (`dMIN`, `dMAX`, `dRES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSubrange {
    pub min: u32,
    pub max: u32,
    pub res: u32,
}

impl RateSubrange {
    pub fn supports(self, rate: u32) -> bool {
        if rate < self.min || rate > self.max {
            return false;
        }
        if self.min == self.max {
            return rate == self.min;
        }
        if self.res == 0 {
            return true;
        }
        (rate - self.min).is_multiple_of(self.res)
    }
}

/// Discovered clock-route capabilities for a UAC2 DAC.
#[derive(Debug, Clone)]
pub struct ProbedClockRoute {
    pub route: ClockRoute,
    pub subranges: Vec<RateSubrange>,
    pub discrete_rates: Vec<u32>,
}

impl ProbedClockRoute {
    pub fn supports(&self, rate: u32) -> bool {
        self.subranges.iter().any(|s| s.supports(rate)) || self.discrete_rates.contains(&rate)
    }
}

/// Hardware volume capabilities & live state (in 1/256 dB signed Q8.8 units).
///
/// Tracks the user's chosen listening volume (`user_raw`) separately from any per-track
/// ReplayGain offset (`replay_gain_db`), so ReplayGain operates through the DAC's own hardware
/// mixer while keeping PCM 100% bit-perfect and keeping the user's volume slider stable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeControlInfo {
    pub unit_id: u8,
    pub ac_interface: u8,
    pub uac_version: UacVersion,
    /// Channels controlled (`[0]` for master, or e.g. `[1, 2]` for L+R).
    pub use_master: bool,
    pub ch_count: u8,
    pub channels: [u8; 8],
    pub has_mute: bool,
    pub mute_master: bool,
    pub min_raw: i16,
    pub max_raw: i16,
    pub res_raw: i16,
    /// User's base hardware volume (Q8.8 dB), before ReplayGain offset.
    pub user_raw: i16,
    /// Active per-track ReplayGain offset in dB (`0.0` when off or untagged).
    pub replay_gain_db: f32,
    /// Actual effective hardware volume currently written to the DAC (Q8.8 dB).
    pub cur_raw: i16,
}

impl VolumeControlInfo {
    pub fn min_db(self) -> f32 {
        f32::from(self.min_raw) / 256.0
    }

    pub fn max_db(self) -> f32 {
        f32::from(self.max_raw) / 256.0
    }

    pub fn user_db(self) -> f32 {
        f32::from(self.user_raw) / 256.0
    }

    pub fn current_db(self) -> f32 {
        f32::from(self.cur_raw) / 256.0
    }

    /// User's normalized slider position `[0.0, 1.0]` (stable across ReplayGain track changes).
    pub fn normalized(self) -> f32 {
        let span = f32::from(self.max_raw) - f32::from(self.min_raw);
        if span <= 0.0 {
            return 1.0;
        }
        ((f32::from(self.user_raw) - f32::from(self.min_raw)) / span).clamp(0.0, 1.0)
    }

    /// Maps normalized `[0.0, 1.0]` to a valid raw Q8.8 value snapped to `res_raw`.
    pub fn raw_from_normalized(self, norm: f32) -> i16 {
        let n = norm.clamp(0.0, 1.0);
        let min = i32::from(self.min_raw);
        let max = i32::from(self.max_raw);
        if max <= min {
            return self.max_raw;
        }
        let target = min + ((max - min) as f32 * n).round() as i32;
        let res = i32::from(self.res_raw).max(1);
        let snapped = min + ((target - min + res / 2) / res) * res;
        snapped.clamp(min, max) as i16
    }

    /// Computes effective raw Q8.8 volume (`user_raw + replay_gain_db`), snapped downward to
    /// `res_raw` (never louder than requested) and clamped to `[min_raw, max_raw]`.
    pub fn effective_raw(self, user_raw: i16, replay_gain_db: f32) -> i16 {
        let min = i32::from(self.min_raw);
        let max = i32::from(self.max_raw);
        if max <= min {
            return self.max_raw;
        }
        if user_raw <= self.min_raw {
            return self.min_raw;
        }
        let rg = if replay_gain_db.is_finite() {
            (replay_gain_db.clamp(-40.0, 24.0) * 256.0).floor() as i32
        } else {
            0
        };
        let combined = (i32::from(user_raw) + rg).clamp(min, max);
        let res = i32::from(self.res_raw).max(1);
        // Round down (floor) to hardware step so positive ReplayGain never overshoots.
        let snapped = min + ((combined - min) / res) * res;
        snapped.clamp(min, max) as i16
    }

    fn volume_channels(&self) -> &[u8] {
        if self.use_master {
            &[0]
        } else {
            &self.channels[..usize::from(self.ch_count)]
        }
    }
}

/// Parses a UAC2 `CS_SAM_FREQ_CONTROL` `RANGE` payload into `(subranges, expanded_rates)`.
pub fn parse_uac2_rate_range(buf: &[u8]) -> (Vec<RateSubrange>, Vec<u32>) {
    if buf.len() < 2 {
        return (Vec::new(), Vec::new());
    }
    let count = u16::from_le_bytes([buf[0], buf[1]]) as usize;
    let mut subranges = Vec::new();
    let mut rates = Vec::new();

    for i in 0..count {
        let base = 2 + i * 12;
        if base + 12 > buf.len() {
            break;
        }
        let min = u32::from_le_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
        let max = u32::from_le_bytes([buf[base + 4], buf[base + 5], buf[base + 6], buf[base + 7]]);
        let res = u32::from_le_bytes([buf[base + 8], buf[base + 9], buf[base + 10], buf[base + 11]]);
        if min == 0 || max < min {
            continue;
        }
        let sr = RateSubrange { min, max, res };
        subranges.push(sr);

        if min == max {
            rates.push(min);
        } else if res > 0 && (max - min) / res <= 64 {
            let mut r = min;
            while r <= max {
                rates.push(r);
                match r.checked_add(res) {
                    Some(next) => r = next,
                    None => break,
                }
            }
        } else {
            for &std_rate in STANDARD_RATES {
                if sr.supports(std_rate) {
                    rates.push(std_rate);
                }
            }
        }
    }

    rates.sort_unstable();
    rates.dedup();
    (subranges, rates)
}

/// Parses a UAC2 `FU_VOLUME_CONTROL` `RANGE` payload into `(min_raw, max_raw, res_raw)`.
pub fn parse_uac2_volume_range(buf: &[u8]) -> Option<(i16, i16, i16)> {
    if buf.len() < 8 {
        return None;
    }
    let count = u16::from_le_bytes([buf[0], buf[1]]) as usize;
    if count == 0 {
        return None;
    }
    let mut min_raw = i16::MAX;
    let mut max_raw = i16::MIN;
    let mut res_raw = 0i16;
    for i in 0..count {
        let base = 2 + i * 6;
        if base + 6 > buf.len() {
            break;
        }
        let mn = i16::from_le_bytes([buf[base], buf[base + 1]]);
        let mx = i16::from_le_bytes([buf[base + 2], buf[base + 3]]);
        let rs = i16::from_le_bytes([buf[base + 4], buf[base + 5]]);
        if mn < min_raw {
            min_raw = mn;
        }
        if mx > max_raw {
            max_raw = mx;
        }
        if res_raw == 0 && rs > 0 {
            res_raw = rs;
        }
    }
    if min_raw <= max_raw {
        Some((min_raw, max_raw, res_raw.max(1)))
    } else {
        None
    }
}

/// Probes all UAC2 clock routes to discover every hardware-supported sample rate.
pub fn probe_uac2_clocks(fd: RawFd, parsed: &ParsedUsbAudio) -> Vec<ProbedClockRoute> {
    let mut probed = Vec::new();
    for &route in &parsed.clock_routes {
        let w_index = (u16::from(route.clock_source.id) << 8) | u16::from(parsed.ac_interface);
        // Step 1: read 2-byte count header first (Linux snd-usb-audio idiom for picky firmware).
        let mut hdr = [0u8; 2];
        let full_len = match usbfs::control_transfer(
            fd,
            REQ_CLASS_IF_IN,
            UAC2_RANGE,
            CS_SAM_FREQ_CONTROL,
            w_index,
            &mut hdr,
            CTRL_TIMEOUT_MS,
        ) {
            Ok(2) => {
                let count = u16::from_le_bytes(hdr) as usize;
                (2 + count.saturating_mul(12)).clamp(14, 512)
            }
            _ => 256,
        };

        let mut buf = vec![0u8; full_len];
        let (subranges, mut discrete_rates) = match usbfs::control_transfer(
            fd,
            REQ_CLASS_IF_IN,
            UAC2_RANGE,
            CS_SAM_FREQ_CONTROL,
            w_index,
            &mut buf,
            CTRL_TIMEOUT_MS,
        ) {
            Ok(n) if n >= 14 => parse_uac2_rate_range(&buf[..n]),
            _ => (Vec::new(), Vec::new()),
        };

        // Fallback for fixed-clock DACs that implement CUR but stall RANGE.
        if discrete_rates.is_empty()
            && route.clock_source.freq_readable()
            && let Some(cur) = get_uac2_sample_rate(fd, parsed.ac_interface, route.clock_source.id)
        {
            discrete_rates.push(cur);
        }

        probed.push(ProbedClockRoute {
            route,
            subranges,
            discrete_rates,
        });
    }
    probed
}

/// Collects the deduplicated, sorted list of all hardware sample rates advertised by a DAC.
pub fn collect_supported_rates(
    parsed: &ParsedUsbAudio,
    probed_clocks: &[ProbedClockRoute],
) -> Vec<u32> {
    let mut rates = Vec::new();
    match parsed.uac_version {
        UacVersion::Uac2 => {
            for c in probed_clocks {
                rates.extend_from_slice(&c.discrete_rates);
            }
        }
        UacVersion::Uac1 => {
            for alt in &parsed.alt_settings {
                match &alt.uac1_rates {
                    Some(Uac1SampleRates::Discrete(list)) => rates.extend_from_slice(list),
                    Some(Uac1SampleRates::Continuous { min, max }) => {
                        for &r in STANDARD_RATES {
                            if r >= *min && r <= *max {
                                rates.push(r);
                            }
                        }
                    }
                    None => {}
                }
            }
        }
    }
    rates.sort_unstable();
    rates.dedup();
    rates
}

/// Configures the USB DAC hardware to the EXACT `sample_rate` and alternate setting `alt`.
/// Never resamples; verifies the DAC accepted the requested sample rate on UAC2.
pub fn configure_hardware_stream(
    fd: RawFd,
    parsed: &ParsedUsbAudio,
    probed_clocks: &[ProbedClockRoute],
    alt: &StreamAltSetting,
    sample_rate: u32,
) -> Result<u32, UsbAudioError> {
    // 1. Always park the streaming interface in zero-bandwidth alt 0 first.
    let _ = usbfs::set_interface(fd, alt.interface_number, 0);

    match parsed.uac_version {
        UacVersion::Uac2 => {
            let clock = probed_clocks
                .iter()
                .find(|c| c.supports(sample_rate))
                .or_else(|| probed_clocks.first())
                .ok_or_else(|| UsbAudioError::ControlFailed("no UAC2 clock source found".into()))?;

            // Route clock selector if needed.
            if let Some((sel_id, pin)) = clock.route.selector {
                let mut pin_buf = [pin];
                let w_index = (u16::from(sel_id) << 8) | u16::from(parsed.ac_interface);
                usbfs::control_transfer(
                    fd,
                    REQ_CLASS_IF_OUT,
                    UAC2_CUR,
                    CX_CLOCK_SELECTOR_CONTROL,
                    w_index,
                    &mut pin_buf,
                    CTRL_TIMEOUT_MS,
                )
                .map_err(|e| UsbAudioError::ControlFailed(format!("clock selector {sel_id} pin {pin}: {e}")))?;
            }

            let clock_id = clock.route.clock_source.id;
            let pre_set_ok = set_uac2_sample_rate(fd, parsed.ac_interface, clock_id, sample_rate).is_ok();

            // 2. Activate the chosen streaming altsetting.
            usbfs::set_interface(fd, alt.interface_number, alt.alt_setting).map_err(|e| {
                UsbAudioError::ControlFailed(format!(
                    "set_interface({}, alt={}): {e}",
                    alt.interface_number, alt.alt_setting
                ))
            })?;

            // 3. Verify clock rate; some DACs only accept SET_CUR after SET_INTERFACE.
            let mut actual = get_uac2_sample_rate(fd, parsed.ac_interface, clock_id);
            if !pre_set_ok || actual != Some(sample_rate) {
                set_uac2_sample_rate(fd, parsed.ac_interface, clock_id, sample_rate)?;
                actual = get_uac2_sample_rate(fd, parsed.ac_interface, clock_id);
            }

            // Wait briefly for PLL lock if the DAC exposes CS_CLOCK_VALID_CONTROL.
            if clock.route.clock_source.has_validity_control() {
                for _ in 0..10 {
                    if is_uac2_clock_valid(fd, parsed.ac_interface, clock_id) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }

            let confirmed = actual.unwrap_or(sample_rate);
            if confirmed != sample_rate {
                let _ = usbfs::set_interface(fd, alt.interface_number, 0);
                return Err(UsbAudioError::SampleRateRejected {
                    requested: sample_rate,
                    actual: confirmed,
                });
            }
            Ok(confirmed)
        }
        UacVersion::Uac1 => {
            // Activate altsetting first (UAC1 requires active endpoint before EP SET_CUR).
            usbfs::set_interface(fd, alt.interface_number, alt.alt_setting).map_err(|e| {
                UsbAudioError::ControlFailed(format!(
                    "set_interface({}, alt={}): {e}",
                    alt.interface_number, alt.alt_setting
                ))
            })?;

            if alt.out_endpoint.has_sampling_freq_control {
                let b = sample_rate.to_le_bytes();
                let mut rate_bytes = [b[0], b[1], b[2]];
                usbfs::control_transfer(
                    fd,
                    REQ_CLASS_EP_OUT,
                    UAC1_SET_CUR,
                    EP_SAMPLING_FREQ_CONTROL,
                    u16::from(alt.out_endpoint.address),
                    &mut rate_bytes,
                    CTRL_TIMEOUT_MS,
                )
                .map_err(|e| UsbAudioError::ControlFailed(format!("UAC1 SET_CUR {sample_rate} Hz: {e}")))?;

                // Best-effort readback verification (not all UAC1 DACs implement GET_CUR).
                let mut readback = [0u8; 3];
                if let Ok(3) = usbfs::control_transfer(
                    fd,
                    REQ_CLASS_EP_IN,
                    UAC1_GET_CUR,
                    EP_SAMPLING_FREQ_CONTROL,
                    u16::from(alt.out_endpoint.address),
                    &mut readback,
                    CTRL_TIMEOUT_MS,
                ) {
                    let actual = u32::from_le_bytes([readback[0], readback[1], readback[2], 0]);
                    if actual > 0 && actual != sample_rate {
                        let _ = usbfs::set_interface(fd, alt.interface_number, 0);
                        return Err(UsbAudioError::SampleRateRejected {
                            requested: sample_rate,
                            actual,
                        });
                    }
                }
            }
            Ok(sample_rate)
        }
    }
}

fn set_uac2_sample_rate(fd: RawFd, ac_if: u8, clock_id: u8, sample_rate: u32) -> Result<(), UsbAudioError> {
    let mut rate_bytes = sample_rate.to_le_bytes();
    let w_index = (u16::from(clock_id) << 8) | u16::from(ac_if);
    usbfs::control_transfer(
        fd,
        REQ_CLASS_IF_OUT,
        UAC2_CUR,
        CS_SAM_FREQ_CONTROL,
        w_index,
        &mut rate_bytes,
        CTRL_TIMEOUT_MS,
    )
    .map_err(|e| UsbAudioError::ControlFailed(format!("UAC2 SET_CUR {sample_rate} Hz: {e}")))?;
    Ok(())
}

fn get_uac2_sample_rate(fd: RawFd, ac_if: u8, clock_id: u8) -> Option<u32> {
    let mut buf = [0u8; 4];
    let w_index = (u16::from(clock_id) << 8) | u16::from(ac_if);
    match usbfs::control_transfer(
        fd,
        REQ_CLASS_IF_IN,
        UAC2_CUR,
        CS_SAM_FREQ_CONTROL,
        w_index,
        &mut buf,
        CTRL_TIMEOUT_MS,
    ) {
        Ok(4) => {
            let r = u32::from_le_bytes(buf);
            (r > 0).then_some(r)
        }
        _ => None,
    }
}

fn is_uac2_clock_valid(fd: RawFd, ac_if: u8, clock_id: u8) -> bool {
    let mut buf = [0u8; 1];
    let w_index = (u16::from(clock_id) << 8) | u16::from(ac_if);
    matches!(
        usbfs::control_transfer(
            fd,
            REQ_CLASS_IF_IN,
            UAC2_CUR,
            CS_CLOCK_VALID_CONTROL,
            w_index,
            &mut buf,
            CTRL_TIMEOUT_MS,
        ),
        Ok(1) if buf[0] != 0
    )
}

/// Probes the playback Feature Unit for hardware volume range, unmutes the DAC, and reads its
/// current hardware gain. Returns `None` if the DAC exposes no hardware volume control.
pub fn probe_hardware_volume(
    fd: RawFd,
    ac_if: u8,
    ver: UacVersion,
    fu: &FeatureUnit,
) -> Option<VolumeControlInfo> {
    // Always unmute the Feature Unit if it supports mute so playback is audible.
    if fu.has_mute() {
        let _ = set_feature_mute(fd, ac_if, ver, fu, false);
    }
    if !fu.has_volume() {
        return None;
    }

    let use_master = fu.master_volume;
    let mut channels = [0u8; 8];
    let ch_count = if use_master {
        0
    } else {
        let n = fu.channel_volumes.len().min(8);
        channels[..n].copy_from_slice(&fu.channel_volumes[..n]);
        n as u8
    };
    let probe_ch = if use_master { 0u8 } else { channels[0] };
    let w_index = (u16::from(fu.unit_id) << 8) | u16::from(ac_if);
    let w_value = FU_VOLUME_CONTROL | u16::from(probe_ch);

    let (min_raw, max_raw, res_raw) = match ver {
        UacVersion::Uac2 => {
            let mut buf = [0u8; 64];
            let n = usbfs::control_transfer(
                fd,
                REQ_CLASS_IF_IN,
                UAC2_RANGE,
                w_value,
                w_index,
                &mut buf,
                CTRL_TIMEOUT_MS,
            )
            .ok()?;
            parse_uac2_volume_range(&buf[..n])?
        }
        UacVersion::Uac1 => {
            let mut mn = [0u8; 2];
            let mut mx = [0u8; 2];
            let mut rs = [0u8; 2];
            usbfs::control_transfer(fd, REQ_CLASS_IF_IN, UAC1_GET_MIN, w_value, w_index, &mut mn, CTRL_TIMEOUT_MS)
                .ok()?;
            usbfs::control_transfer(fd, REQ_CLASS_IF_IN, UAC1_GET_MAX, w_value, w_index, &mut mx, CTRL_TIMEOUT_MS)
                .ok()?;
            let res = usbfs::control_transfer(
                fd,
                REQ_CLASS_IF_IN,
                UAC1_GET_RES,
                w_value,
                w_index,
                &mut rs,
                CTRL_TIMEOUT_MS,
            )
            .ok()
            .filter(|&n| n == 2)
            .map(|_| i16::from_le_bytes(rs).max(1))
            .unwrap_or(256);
            let min_r = i16::from_le_bytes(mn);
            let max_r = i16::from_le_bytes(mx);
            if min_r > max_r {
                return None;
            }
            (min_r, max_r, res)
        }
    };

    let mut cur_buf = [0u8; 2];
    let cur_req = if ver == UacVersion::Uac2 { UAC2_CUR } else { UAC1_GET_CUR };
    let cur_raw = usbfs::control_transfer(fd, REQ_CLASS_IF_IN, cur_req, w_value, w_index, &mut cur_buf, CTRL_TIMEOUT_MS)
        .ok()
        .filter(|&n| n == 2)
        .map(|_| i16::from_le_bytes(cur_buf).clamp(min_raw, max_raw))
        .unwrap_or(max_raw);

    let mut info = VolumeControlInfo {
        unit_id: fu.unit_id,
        ac_interface: ac_if,
        uac_version: ver,
        use_master,
        ch_count,
        channels,
        has_mute: fu.has_mute(),
        mute_master: fu.master_mute,
        min_raw,
        max_raw,
        res_raw,
        user_raw: cur_raw,
        replay_gain_db: 0.0,
        cur_raw,
    };

    // If the DAC powered up completely silenced (`cur_raw == min_raw`), lift it to a safe audible
    // level (50% of its dB range) so the user hears music without risking an ear-blasting 0 dB jump.
    if info.cur_raw <= info.min_raw && info.max_raw > info.min_raw {
        let safe = info.raw_from_normalized(0.5);
        if set_hardware_volume_raw(fd, &mut info, safe).is_ok() {
            info.user_raw = safe;
            info.cur_raw = safe;
        }
    }

    Some(info)
}

/// Sets the DAC's hardware volume to `raw_q8_8` across master or all stereo channels.
pub fn set_hardware_volume_raw(
    fd: RawFd,
    info: &mut VolumeControlInfo,
    raw_q8_8: i16,
) -> Result<(), UsbAudioError> {
    let clamped = raw_q8_8.clamp(info.min_raw, info.max_raw);
    let w_index = (u16::from(info.unit_id) << 8) | u16::from(info.ac_interface);
    let req = if info.uac_version == UacVersion::Uac2 { UAC2_CUR } else { UAC1_SET_CUR };
    for &ch in info.volume_channels() {
        let mut bytes = clamped.to_le_bytes();
        usbfs::control_transfer(
            fd,
            REQ_CLASS_IF_OUT,
            req,
            FU_VOLUME_CONTROL | u16::from(ch),
            w_index,
            &mut bytes,
            CTRL_TIMEOUT_MS,
        )
        .map_err(|e| UsbAudioError::ControlFailed(format!("set hardware volume ch{ch}: {e}")))?;
    }
    info.cur_raw = clamped;
    Ok(())
}

fn set_feature_mute(
    fd: RawFd,
    ac_if: u8,
    ver: UacVersion,
    fu: &FeatureUnit,
    muted: bool,
) -> Result<(), UsbAudioError> {
    let w_index = (u16::from(fu.unit_id) << 8) | u16::from(ac_if);
    let req = if ver == UacVersion::Uac2 { UAC2_CUR } else { UAC1_SET_CUR };
    let val = u8::from(muted);
    if fu.master_mute {
        let mut buf = [val];
        let _ = usbfs::control_transfer(fd, REQ_CLASS_IF_OUT, req, FU_MUTE_CONTROL, w_index, &mut buf, CTRL_TIMEOUT_MS);
    }
    for &ch in &fu.channel_mutes {
        let mut buf = [val];
        let _ = usbfs::control_transfer(
            fd,
            REQ_CLASS_IF_OUT,
            req,
            FU_MUTE_CONTROL | u16::from(ch),
            w_index,
            &mut buf,
            CTRL_TIMEOUT_MS,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uac2_discrete_and_stepped_rate_ranges() {
        // 8 discrete audiophile rates (EPZ TP35 Pro / CS43198 style: 44.1k .. 384k).
        let expected = [44_100u32, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000];
        let mut buf = Vec::new();
        buf.extend_from_slice(&(expected.len() as u16).to_le_bytes());
        for r in expected {
            buf.extend_from_slice(&r.to_le_bytes());
            buf.extend_from_slice(&r.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
        let (subranges, rates) = parse_uac2_rate_range(&buf);
        assert_eq!(subranges.len(), 8);
        assert_eq!(rates, expected);
    }

    #[test]
    fn parses_uac2_volume_range_and_snaps_normalized_steps() {
        // -60 dB (-15360) to 0 dB (0) in 0.5 dB (128) steps.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&(-15360i16).to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&128i16.to_le_bytes());

        let (mn, mx, rs) = parse_uac2_volume_range(&buf).unwrap();
        assert_eq!((mn, mx, rs), (-15360, 0, 128));

        let info = VolumeControlInfo {
            unit_id: 10,
            ac_interface: 0,
            uac_version: UacVersion::Uac2,
            use_master: true,
            ch_count: 0,
            channels: [0; 8],
            has_mute: true,
            mute_master: true,
            min_raw: mn,
            max_raw: mx,
            res_raw: rs,
            user_raw: -7680,
            replay_gain_db: 0.0,
            cur_raw: -7680,
        };
        assert!((info.current_db() - (-30.0)).abs() < 0.01);
        assert!((info.normalized() - 0.5).abs() < 0.01);
        assert_eq!(info.raw_from_normalized(0.0), -15360);
        assert_eq!(info.raw_from_normalized(1.0), 0);
        assert_eq!(info.raw_from_normalized(0.5), -7680);
        // Hardware ReplayGain offset: -30.0 dB + (-6.5 dB) = -36.5 dB (-9344), clamped at [min, max]
        // and sanity-clamped to [-40, +24] dB so malformed tags cannot jump further than +24 dB.
        assert_eq!(info.effective_raw(-7680, -6.5), -9344);
        assert_eq!(info.effective_raw(-7680, 40.0), -1536); // -30 + 24 = -6.0 dB
        assert_eq!(info.effective_raw(-2560, 20.0), 0); // -10 + 20 -> clamped at max_raw (0 dB)
        assert_eq!(info.effective_raw(-7680, -40.0), -15360);
        // Muted user level stays muted even with positive ReplayGain.
        assert_eq!(info.effective_raw(-15360, 12.0), -15360);
    }
}

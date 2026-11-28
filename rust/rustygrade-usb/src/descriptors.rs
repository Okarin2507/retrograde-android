//! Pure-Rust USB Audio Class 1.0 (UAC1) and 2.0 (UAC2) descriptor parser.
//!
//! Parses raw USB descriptors returned by Android's `UsbDeviceConnection.getRawDescriptors()`
//! (device descriptor followed by one or more full configuration descriptors) to discover:
//! - AudioControl (AC) interface & UAC version
//! - UAC2 Clock Sources, Clock Selectors, and Clock Multipliers
//! - Playback Feature Unit (hardware DAC volume & mute controls)
//! - AudioStreaming (AS) OUT interfaces, alternate settings, PCM Type I formats,
//!   isochronous OUT endpoints, and explicit asynchronous feedback IN endpoints.

use crate::UsbAudioError;

// Standard USB descriptor types.
const DESC_DEVICE: u8 = 0x01;
const DESC_CONFIGURATION: u8 = 0x02;
const DESC_INTERFACE: u8 = 0x04;
const DESC_ENDPOINT: u8 = 0x05;
const DESC_CS_INTERFACE: u8 = 0x24;
const DESC_CS_ENDPOINT: u8 = 0x25;

// USB Audio Class codes.
const CLASS_AUDIO: u8 = 0x01;
const SUBCLASS_AUDIOCONTROL: u8 = 0x01;
const SUBCLASS_AUDIOSTREAMING: u8 = 0x02;
const PROTO_UAC2: u8 = 0x20;

// AudioControl class-specific descriptor subtypes.
const AC_INPUT_TERMINAL: u8 = 0x02;
const AC_OUTPUT_TERMINAL: u8 = 0x03;
const AC_MIXER_UNIT: u8 = 0x04;
const AC_SELECTOR_UNIT: u8 = 0x05;
const AC_FEATURE_UNIT: u8 = 0x06;
const AC_CLOCK_SOURCE: u8 = 0x0A;
const AC_CLOCK_SELECTOR: u8 = 0x0B;
const AC_CLOCK_MULTIPLIER: u8 = 0x0C;

// AudioStreaming class-specific descriptor subtypes.
const AS_GENERAL: u8 = 0x01;
const AS_FORMAT_TYPE: u8 = 0x02;

// Class-specific endpoint descriptor subtypes.
const EP_GENERAL: u8 = 0x01;

// Format types & tags.
const FORMAT_TYPE_I: u8 = 0x01;
const UAC1_FORMAT_PCM: u16 = 0x0001;
const UAC2_FORMAT_PCM_BIT: u32 = 1 << 0;

// Terminal types.
const TERMINAL_USB_STREAMING: u16 = 0x0101;

/// USB Audio Class version reported by the AudioControl interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UacVersion {
    Uac1,
    Uac2,
}

/// Isochronous endpoint synchronization mode (`bmAttributes` bits 3..2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoSyncType {
    None,
    Asynchronous,
    Adaptive,
    Synchronous,
}

impl IsoSyncType {
    fn from_ep_attrs(bm_attributes: u8) -> Self {
        match (bm_attributes >> 2) & 0x03 {
            0x01 => Self::Asynchronous,
            0x02 => Self::Adaptive,
            0x03 => Self::Synchronous,
            _ => Self::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Asynchronous => "ASYNC",
            Self::Adaptive => "ADAPTIVE",
            Self::Synchronous => "SYNC",
            Self::None => "ISO",
        }
    }
}

/// Sample rate set declared directly in a UAC1 `FORMAT_TYPE` descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uac1SampleRates {
    Discrete(Vec<u32>),
    Continuous { min: u32, max: u32 },
}

impl Uac1SampleRates {
    pub fn supports(&self, rate: u32) -> bool {
        match self {
            Self::Discrete(rates) => rates.contains(&rate),
            Self::Continuous { min, max } => rate >= *min && rate <= *max,
        }
    }
}

/// Isochronous endpoint descriptor summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoEndpoint {
    pub address: u8,
    pub sync_type: IsoSyncType,
    /// Decoded total bytes per frame/microframe including USB 2.0 high-bandwidth multiplier.
    pub max_packet_size: u16,
    pub interval: u8,
    pub synch_address: u8,
    /// UAC1 `EP_GENERAL` bit 0: whether endpoint accepts `SAMPLING_FREQ_CONTROL`.
    pub has_sampling_freq_control: bool,
}

/// Explicit feedback IN endpoint paired with an asynchronous OUT endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackEndpoint {
    pub address: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

/// One playable PCM alternate setting on an AudioStreaming OUT interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamAltSetting {
    pub interface_number: u8,
    pub alt_setting: u8,
    pub terminal_link: u8,
    pub channels: u8,
    /// Bytes per channel sample on the USB wire (`bSubframeSize` / `bSubslotSize`): 2, 3, or 4.
    pub subslot_bytes: u8,
    /// Significant bits per sample (`bBitResolution`): e.g., 16, 24, or 32.
    pub bit_resolution: u8,
    pub out_endpoint: IsoEndpoint,
    pub feedback_endpoint: Option<FeedbackEndpoint>,
    /// Populated for UAC1; UAC2 rates are queried dynamically from the Clock Source.
    pub uac1_rates: Option<Uac1SampleRates>,
}

/// UAC2 Clock Source entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSource {
    pub id: u8,
    pub attributes: u8,
    pub controls: u8,
}

impl ClockSource {
    /// True if `CS_SAM_FREQ_CONTROL` is host-programmable (bits 1..0 == 0b11).
    pub fn freq_programmable(self) -> bool {
        (self.controls & 0x03) == 0x03
    }

    /// True if `CS_SAM_FREQ_CONTROL` is readable (bits 1..0 != 0b00).
    pub fn freq_readable(self) -> bool {
        (self.controls & 0x03) != 0x00
    }

    /// True if `CS_CLOCK_VALID_CONTROL` is readable (bits 3..2 != 0b00).
    pub fn has_validity_control(self) -> bool {
        ((self.controls >> 2) & 0x03) != 0x00
    }
}

/// UAC2 Clock Selector entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockSelector {
    pub id: u8,
    pub source_ids: Vec<u8>,
    pub controls: u8,
}

/// UAC2 Clock Multiplier entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockMultiplier {
    pub id: u8,
    pub source_id: u8,
}

/// A clock source reachable from a playback terminal (directly or through a Clock Selector).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockRoute {
    pub clock_source: ClockSource,
    /// If routed through a `ClockSelector`, `(selector_id, 1_based_pin_index)`.
    pub selector: Option<(u8, u8)>,
}

/// Feature Unit on the playback path (used for hardware DAC volume & mute).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureUnit {
    pub unit_id: u8,
    pub source_id: u8,
    pub master_mute: bool,
    pub master_volume: bool,
    /// Per-channel indices (1-based, e.g. `[1, 2]` for L/R) that support R/W mute.
    pub channel_mutes: Vec<u8>,
    /// Per-channel indices (1-based, e.g. `[1, 2]` for L/R) that support R/W volume.
    pub channel_volumes: Vec<u8>,
}

impl FeatureUnit {
    pub fn has_volume(&self) -> bool {
        self.master_volume || !self.channel_volumes.is_empty()
    }

    pub fn has_mute(&self) -> bool {
        self.master_mute || !self.channel_mutes.is_empty()
    }
}

#[derive(Debug, Clone)]
struct InputTerminal {
    id: u8,
    terminal_type: u16,
    clock_source_id: u8,
}

#[derive(Debug, Clone)]
struct OutputTerminal {
    terminal_type: u16,
    source_id: u8,
}

#[derive(Debug, Clone)]
struct AudioTopology {
    input_terminals: Vec<InputTerminal>,
    output_terminals: Vec<OutputTerminal>,
    feature_units: Vec<FeatureUnit>,
    /// Generic upstream links `(unit_id, source_ids)` for Mixer/Selector traversal.
    extra_sources: Vec<(u8, Vec<u8>)>,
    clock_sources: Vec<ClockSource>,
    clock_selectors: Vec<ClockSelector>,
    clock_multipliers: Vec<ClockMultiplier>,
}

/// Complete parsed USB audio playback configuration for a single USB device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUsbAudio {
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_usb: u16,
    pub config_value: u8,
    pub uac_version: UacVersion,
    pub ac_interface: u8,
    /// All playable PCM alternate settings (grouped on the chosen playback AS interface).
    pub alt_settings: Vec<StreamAltSetting>,
    /// UAC2 clock routes reachable from the playback terminal (in preference order).
    pub clock_routes: Vec<ClockRoute>,
    /// Feature unit on the playback path, if the DAC exposes hardware volume/mute.
    pub feature_unit: Option<FeatureUnit>,
}

impl ParsedUsbAudio {
    /// Interface number of the selected AudioStreaming OUT interface.
    pub fn as_interface(&self) -> u8 {
        self.alt_settings[0].interface_number
    }

    /// All interface numbers that must be claimed exclusively (AC + AS OUT).
    pub fn required_interfaces(&self) -> Vec<u8> {
        let mut ifaces = vec![self.ac_interface];
        let as_if = self.as_interface();
        if !ifaces.contains(&as_if) {
            ifaces.push(as_if);
        }
        ifaces
    }
}

/// Parses raw USB descriptors (Device + Configuration descriptors) into a playable audio topology.
///
/// If `preferred_config` is `Some(val)`, that configuration is preferred when multiple USB
/// configurations exist; otherwise the first configuration with a playable PCM OUT stream wins.
pub fn parse_raw_descriptors(
    raw: &[u8],
    preferred_config: Option<u8>,
) -> Result<ParsedUsbAudio, UsbAudioError> {
    if raw.len() < 18 {
        return Err(UsbAudioError::InvalidDescriptors("descriptor buffer shorter than 18 bytes".into()));
    }

    let mut vendor_id = 0u16;
    let mut product_id = 0u16;
    let mut bcd_usb = 0u16;

    // Split raw buffer into Device descriptor header + individual Configuration blobs.
    let mut offset = 0usize;
    let mut configs: Vec<(u8, &[u8])> = Vec::new();
    while offset + 2 <= raw.len() {
        let b_len = raw[offset] as usize;
        let b_type = raw[offset + 1];
        if b_len < 2 || offset + b_len > raw.len() {
            break;
        }
        match b_type {
            DESC_DEVICE if b_len >= 18 => {
                bcd_usb = u16::from_le_bytes([raw[offset + 2], raw[offset + 3]]);
                vendor_id = u16::from_le_bytes([raw[offset + 8], raw[offset + 9]]);
                product_id = u16::from_le_bytes([raw[offset + 10], raw[offset + 11]]);
                offset += b_len;
            }
            DESC_CONFIGURATION if b_len >= 9 => {
                let total_len = u16::from_le_bytes([raw[offset + 2], raw[offset + 3]]) as usize;
                let config_val = raw[offset + 5];
                let end = (offset + total_len).min(raw.len());
                configs.push((config_val, &raw[offset..end]));
                offset = if total_len >= b_len { end } else { offset + b_len };
            }
            _ => {
                offset += b_len;
            }
        }
    }

    if configs.is_empty() {
        return Err(UsbAudioError::InvalidDescriptors("no USB configuration descriptor found".into()));
    }

    // Try preferred config first (if present), then any remaining configurations.
    let mut ordered: Vec<(u8, &[u8])> = Vec::with_capacity(configs.len());
    if let Some(pref) = preferred_config {
        for &c in &configs {
            if c.0 == pref {
                ordered.push(c);
            }
        }
    }
    for &c in &configs {
        if !ordered.iter().any(|o| o.0 == c.0 && o.1.as_ptr() == c.1.as_ptr()) {
            ordered.push(c);
        }
    }

    let mut last_err = UsbAudioError::NoAudioStreamingInterface;
    for (config_value, blob) in ordered {
        match parse_configuration(blob, vendor_id, product_id, bcd_usb, config_value) {
            Ok(parsed) => return Ok(parsed),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

#[derive(Default)]
struct PendingAlt {
    interface_number: u8,
    alt_setting: u8,
    uac_version: Option<UacVersion>,
    terminal_link: Option<u8>,
    is_pcm: bool,
    channels: u8,
    subslot_bytes: u8,
    bit_resolution: u8,
    uac1_rates: Option<Uac1SampleRates>,
    out_endpoint: Option<IsoEndpoint>,
    feedback_endpoints: Vec<(FeedbackEndpoint, bool)>, // (ep, is_explicit_feedback_usage)
    last_was_out_ep: bool,
}

impl PendingAlt {
    fn into_stream_alt(self) -> Option<StreamAltSetting> {
        let out_ep = self.out_endpoint?;
        if !self.is_pcm || self.channels == 0 || !matches!(self.subslot_bytes, 2..=4) || self.bit_resolution == 0 {
            return None;
        }
        // Pick explicit feedback IN endpoint matching usage bits or UAC1 bSynchAddress.
        let feedback = self
            .feedback_endpoints
            .iter()
            .find(|(ep, explicit)| *explicit || (out_ep.synch_address != 0 && ep.address == out_ep.synch_address))
            .or_else(|| {
                // Some UAC1/UAC2 async DACs omit usage bits on their sole IN iso endpoint in an OUT altsetting.
                if out_ep.sync_type == IsoSyncType::Asynchronous {
                    self.feedback_endpoints.first()
                } else {
                    None
                }
            })
            .map(|(ep, _)| *ep);

        Some(StreamAltSetting {
            interface_number: self.interface_number,
            alt_setting: self.alt_setting,
            terminal_link: self.terminal_link.unwrap_or(0),
            channels: self.channels,
            subslot_bytes: self.subslot_bytes,
            bit_resolution: self.bit_resolution,
            out_endpoint: out_ep,
            feedback_endpoint: feedback,
            uac1_rates: self.uac1_rates,
        })
    }
}

fn parse_configuration(
    blob: &[u8],
    vendor_id: u16,
    product_id: u16,
    bcd_usb: u16,
    config_value: u8,
) -> Result<ParsedUsbAudio, UsbAudioError> {
    let mut ac_interface: Option<u8> = None;
    let mut uac_version: Option<UacVersion> = None;

    let mut topology = AudioTopology {
        input_terminals: Vec::new(),
        output_terminals: Vec::new(),
        feature_units: Vec::new(),
        extra_sources: Vec::new(),
        clock_sources: Vec::new(),
        clock_selectors: Vec::new(),
        clock_multipliers: Vec::new(),
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        Other,
        AudioControl(UacVersion),
        AudioStreaming(UacVersion),
    }

    let mut section = Section::Other;
    let mut pending_alt: Option<PendingAlt> = None;
    let mut all_alts: Vec<StreamAltSetting> = Vec::new();

    let flush_alt = |pending: &mut Option<PendingAlt>, out: &mut Vec<StreamAltSetting>| {
        if let Some(p) = pending.take()
            && let Some(alt) = p.into_stream_alt()
        {
            out.push(alt);
        }
    };

    let mut offset = 0usize;
    while offset + 2 <= blob.len() {
        let b_len = blob[offset] as usize;
        let b_type = blob[offset + 1];
        if b_len < 2 || offset + b_len > blob.len() {
            break;
        }
        let d = &blob[offset..offset + b_len];
        offset += b_len;

        match b_type {
            DESC_INTERFACE if b_len >= 9 => {
                flush_alt(&mut pending_alt, &mut all_alts);
                let iface_num = d[2];
                let alt_setting = d[3];
                let cls = d[5];
                let subcls = d[6];
                let proto = d[7];

                if cls == CLASS_AUDIO && subcls == SUBCLASS_AUDIOCONTROL {
                    let ver = if proto == PROTO_UAC2 { UacVersion::Uac2 } else { UacVersion::Uac1 };
                    if ac_interface.is_none() {
                        ac_interface = Some(iface_num);
                        uac_version = Some(ver);
                    }
                    section = Section::AudioControl(ver);
                } else if cls == CLASS_AUDIO && subcls == SUBCLASS_AUDIOSTREAMING {
                    let ver = if proto == PROTO_UAC2 {
                        UacVersion::Uac2
                    } else {
                        uac_version.unwrap_or(UacVersion::Uac1)
                    };
                    section = Section::AudioStreaming(ver);
                    if alt_setting > 0 {
                        pending_alt = Some(PendingAlt {
                            interface_number: iface_num,
                            alt_setting,
                            uac_version: Some(ver),
                            ..Default::default()
                        });
                    }
                } else {
                    section = Section::Other;
                }
            }
            DESC_CS_INTERFACE if b_len >= 3 => match section {
                Section::AudioControl(ver) => parse_ac_descriptor(d, ver, &mut topology),
                Section::AudioStreaming(ver) => {
                    if let Some(alt) = pending_alt.as_mut() {
                        parse_as_descriptor(d, alt.uac_version.unwrap_or(ver), alt);
                    }
                }
                Section::Other => {}
            },
            DESC_ENDPOINT if b_len >= 7 => {
                if let Section::AudioStreaming(_) = section
                    && let Some(alt) = pending_alt.as_mut()
                {
                    let addr = d[2];
                    let attrs = d[3];
                    let transfer_type = attrs & 0x03;
                    if transfer_type == 0x01 {
                        // Isochronous endpoint.
                        let raw_mps = u16::from_le_bytes([d[4], d[5]]);
                        let base_mps = raw_mps & 0x07FF;
                        let mult = 1 + ((raw_mps >> 11) & 0x03);
                        let max_packet_size = base_mps.saturating_mul(mult);
                        let interval = d[6].max(1);
                        let synch_address = if b_len >= 9 { d[8] } else { 0 };
                        let is_in = (addr & 0x80) != 0;

                        if !is_in {
                            alt.out_endpoint = Some(IsoEndpoint {
                                address: addr,
                                sync_type: IsoSyncType::from_ep_attrs(attrs),
                                max_packet_size,
                                interval,
                                synch_address,
                                has_sampling_freq_control: false,
                            });
                            alt.last_was_out_ep = true;
                        } else {
                            let usage = (attrs >> 4) & 0x03;
                            let is_explicit_fb = usage == 0x01;
                            alt.feedback_endpoints.push((
                                FeedbackEndpoint {
                                    address: addr,
                                    max_packet_size,
                                    interval,
                                },
                                is_explicit_fb,
                            ));
                            alt.last_was_out_ep = false;
                        }
                    } else {
                        alt.last_was_out_ep = false;
                    }
                }
            }
            DESC_CS_ENDPOINT if b_len >= 4 => {
                if let Section::AudioStreaming(_) = section
                    && let Some(alt) = pending_alt.as_mut()
                    && alt.last_was_out_ep
                    && d[2] == EP_GENERAL
                    && let Some(ep) = alt.out_endpoint.as_mut()
                {
                    // UAC1 CS_ENDPOINT bmAttributes bit 0 = Sampling Frequency Control.
                    if (d[3] & 0x01) != 0 {
                        ep.has_sampling_freq_control = true;
                    }
                }
            }
            _ => {}
        }
    }
    flush_alt(&mut pending_alt, &mut all_alts);

    let ac_if = ac_interface.ok_or(UsbAudioError::NoAudioControlInterface)?;
    let ver = uac_version.unwrap_or(UacVersion::Uac1);
    if all_alts.is_empty() {
        return Err(UsbAudioError::NoAudioStreamingInterface);
    }

    // Group playable altsettings by interface number and pick the best playback interface
    // (prefer one linked to a USB Streaming input terminal with stereo/highest bit depth).
    let chosen_as_if = pick_best_as_interface(&all_alts, &topology);
    let alt_settings: Vec<StreamAltSetting> = all_alts
        .into_iter()
        .filter(|a| a.interface_number == chosen_as_if)
        .collect();

    let terminal_link = alt_settings[0].terminal_link;
    let clock_routes = if ver == UacVersion::Uac2 {
        resolve_clock_routes(terminal_link, &topology)
    } else {
        Vec::new()
    };
    let feature_unit = find_playback_feature_unit(terminal_link, &topology);

    Ok(ParsedUsbAudio {
        vendor_id,
        product_id,
        bcd_usb,
        config_value,
        uac_version: ver,
        ac_interface: ac_if,
        alt_settings,
        clock_routes,
        feature_unit,
    })
}

fn parse_ac_descriptor(d: &[u8], ver: UacVersion, t: &mut AudioTopology) {
    let subtype = d[2];
    match subtype {
        AC_INPUT_TERMINAL => match ver {
            UacVersion::Uac1 if d.len() >= 12 => {
                t.input_terminals.push(InputTerminal {
                    id: d[3],
                    terminal_type: u16::from_le_bytes([d[4], d[5]]),
                    clock_source_id: 0,
                });
            }
            UacVersion::Uac2 if d.len() >= 17 => {
                t.input_terminals.push(InputTerminal {
                    id: d[3],
                    terminal_type: u16::from_le_bytes([d[4], d[5]]),
                    clock_source_id: d[7],
                });
            }
            _ => {}
        },
        AC_OUTPUT_TERMINAL => match ver {
            UacVersion::Uac1 if d.len() >= 9 => {
                t.output_terminals.push(OutputTerminal {
                    terminal_type: u16::from_le_bytes([d[4], d[5]]),
                    source_id: d[7],
                });
            }
            UacVersion::Uac2 if d.len() >= 12 => {
                t.output_terminals.push(OutputTerminal {
                    terminal_type: u16::from_le_bytes([d[4], d[5]]),
                    source_id: d[7],
                });
            }
            _ => {}
        },
        AC_FEATURE_UNIT => match ver {
            UacVersion::Uac1 if d.len() >= 7 => {
                let unit_id = d[3];
                let source_id = d[4];
                let ctrl_size = d[5] as usize;
                if ctrl_size == 0 || ctrl_size > 4 {
                    return;
                }
                // Trailing byte is iFeature (1 byte).
                let controls_bytes = d.len().saturating_sub(7);
                let num_entries = controls_bytes / ctrl_size;
                let mut master_mute = false;
                let mut master_volume = false;
                let mut channel_mutes = Vec::new();
                let mut channel_volumes = Vec::new();
                for ch in 0..num_entries {
                    let base = 6 + ch * ctrl_size;
                    let bits = d[base];
                    let has_mute = (bits & 0x01) != 0;
                    let has_vol = (bits & 0x02) != 0;
                    if ch == 0 {
                        master_mute = has_mute;
                        master_volume = has_vol;
                    } else {
                        if has_mute {
                            channel_mutes.push(ch as u8);
                        }
                        if has_vol {
                            channel_volumes.push(ch as u8);
                        }
                    }
                }
                t.feature_units.push(FeatureUnit {
                    unit_id,
                    source_id,
                    master_mute,
                    master_volume,
                    channel_mutes,
                    channel_volumes,
                });
            }
            UacVersion::Uac2 if d.len() >= 10 => {
                let unit_id = d[3];
                let source_id = d[4];
                // Trailing byte is iFeature (1 byte); each bmaControls entry is 4 bytes LE.
                let controls_bytes = d.len().saturating_sub(6);
                let num_entries = controls_bytes / 4;
                let mut master_mute = false;
                let mut master_volume = false;
                let mut channel_mutes = Vec::new();
                let mut channel_volumes = Vec::new();
                for ch in 0..num_entries {
                    let base = 5 + ch * 4;
                    let bits = u32::from_le_bytes([d[base], d[base + 1], d[base + 2], d[base + 3]]);
                    let mute_rw = (bits & 0x03) == 0x03;
                    let vol_rw = ((bits >> 2) & 0x03) == 0x03;
                    if ch == 0 {
                        master_mute = mute_rw;
                        master_volume = vol_rw;
                    } else {
                        if mute_rw {
                            channel_mutes.push(ch as u8);
                        }
                        if vol_rw {
                            channel_volumes.push(ch as u8);
                        }
                    }
                }
                t.feature_units.push(FeatureUnit {
                    unit_id,
                    source_id,
                    master_mute,
                    master_volume,
                    channel_mutes,
                    channel_volumes,
                });
            }
            _ => {}
        },
        AC_MIXER_UNIT | AC_SELECTOR_UNIT if d.len() >= 6 => {
            let unit_id = d[3];
            let nr_in = d[4] as usize;
            if 5 + nr_in <= d.len() {
                t.extra_sources.push((unit_id, d[5..5 + nr_in].to_vec()));
            }
        }
        AC_CLOCK_SOURCE if ver == UacVersion::Uac2 && d.len() >= 8 => {
            t.clock_sources.push(ClockSource {
                id: d[3],
                attributes: d[4],
                controls: d[5],
            });
        }
        AC_CLOCK_SELECTOR if ver == UacVersion::Uac2 && d.len() >= 7 => {
            let id = d[3];
            let nr_pins = d[4] as usize;
            if 5 + nr_pins < d.len() {
                t.clock_selectors.push(ClockSelector {
                    id,
                    source_ids: d[5..5 + nr_pins].to_vec(),
                    controls: d[5 + nr_pins],
                });
            }
        }
        AC_CLOCK_MULTIPLIER if ver == UacVersion::Uac2 && d.len() >= 7 => {
            t.clock_multipliers.push(ClockMultiplier {
                id: d[3],
                source_id: d[4],
            });
        }
        _ => {}
    }
}

fn parse_as_descriptor(d: &[u8], ver: UacVersion, alt: &mut PendingAlt) {
    let subtype = d[2];
    match subtype {
        AS_GENERAL => match ver {
            UacVersion::Uac1 if d.len() >= 7 => {
                alt.terminal_link = Some(d[3]);
                let format_tag = u16::from_le_bytes([d[5], d[6]]);
                alt.is_pcm = format_tag == UAC1_FORMAT_PCM;
            }
            UacVersion::Uac2 if d.len() >= 16 => {
                alt.terminal_link = Some(d[3]);
                let format_type = d[5];
                let bm_formats = u32::from_le_bytes([d[6], d[7], d[8], d[9]]);
                alt.is_pcm = format_type == FORMAT_TYPE_I && (bm_formats & UAC2_FORMAT_PCM_BIT) != 0;
                alt.channels = d[10];
            }
            _ => {}
        },
        AS_FORMAT_TYPE if d.len() >= 6 && d[3] == FORMAT_TYPE_I => match ver {
            UacVersion::Uac1 if d.len() >= 8 => {
                alt.channels = d[4];
                alt.subslot_bytes = d[5];
                alt.bit_resolution = d[6];
                let sam_freq_type = d[7];
                if sam_freq_type == 0 {
                    if d.len() >= 14 {
                        let min = u32::from_le_bytes([d[8], d[9], d[10], 0]);
                        let max = u32::from_le_bytes([d[11], d[12], d[13], 0]);
                        alt.uac1_rates = Some(Uac1SampleRates::Continuous { min, max });
                    }
                } else {
                    let mut rates = Vec::with_capacity(sam_freq_type as usize);
                    for i in 0..sam_freq_type as usize {
                        let base = 8 + i * 3;
                        if base + 3 <= d.len() {
                            let r = u32::from_le_bytes([d[base], d[base + 1], d[base + 2], 0]);
                            if r > 0 {
                                rates.push(r);
                            }
                        }
                    }
                    if !rates.is_empty() {
                        alt.uac1_rates = Some(Uac1SampleRates::Discrete(rates));
                    }
                }
            }
            UacVersion::Uac2 => {
                alt.subslot_bytes = d[4];
                alt.bit_resolution = d[5];
            }
            _ => {}
        },
        _ => {}
    }
}

fn pick_best_as_interface(alts: &[StreamAltSetting], topology: &AudioTopology) -> u8 {
    let mut best_if = alts[0].interface_number;
    let mut best_score = (0u8, 0u8, 0u8);

    let mut seen: Vec<u8> = Vec::new();
    for a in alts {
        if seen.contains(&a.interface_number) {
            continue;
        }
        seen.push(a.interface_number);
        let group: Vec<&StreamAltSetting> = alts.iter().filter(|x| x.interface_number == a.interface_number).collect();
        let usb_stream_linked = group.iter().any(|x| {
            topology
                .input_terminals
                .iter()
                .any(|it| it.id == x.terminal_link && it.terminal_type == TERMINAL_USB_STREAMING)
        });
        let has_stereo = group.iter().any(|x| x.channels == 2);
        let max_bits = group.iter().map(|x| x.bit_resolution).max().unwrap_or(0);
        let score = (u8::from(usb_stream_linked), u8::from(has_stereo), max_bits);
        if score > best_score {
            best_score = score;
            best_if = a.interface_number;
        }
    }
    best_if
}

fn resolve_clock_routes(terminal_link: u8, topology: &AudioTopology) -> Vec<ClockRoute> {
    let start_clock_id = topology
        .input_terminals
        .iter()
        .find(|it| it.id == terminal_link)
        .map(|it| it.clock_source_id)
        .filter(|&id| id != 0);

    let mut routes = Vec::new();
    if let Some(cid) = start_clock_id {
        collect_clock_routes(cid, None, topology, &mut routes, 0);
    }

    // Fallback: if terminal link didn't point to a recognized clock entity, list all clock sources.
    if routes.is_empty() {
        for &cs in &topology.clock_sources {
            routes.push(ClockRoute {
                clock_source: cs,
                selector: None,
            });
        }
    }

    // Prefer programmable internal clocks first.
    routes.sort_by_key(|r| (u8::from(!r.clock_source.freq_programmable()), r.selector.is_some()));
    routes
}

fn collect_clock_routes(
    entity_id: u8,
    selector: Option<(u8, u8)>,
    topology: &AudioTopology,
    out: &mut Vec<ClockRoute>,
    depth: u8,
) {
    if depth > 8 {
        return;
    }
    if let Some(&cs) = topology.clock_sources.iter().find(|c| c.id == entity_id) {
        if !out.iter().any(|r| r.clock_source.id == cs.id && r.selector == selector) {
            out.push(ClockRoute {
                clock_source: cs,
                selector,
            });
        }
        return;
    }
    if let Some(sel) = topology.clock_selectors.iter().find(|s| s.id == entity_id) {
        for (idx, &src) in sel.source_ids.iter().enumerate() {
            let pin = (idx + 1) as u8;
            let route_sel = selector.or(Some((sel.id, pin)));
            collect_clock_routes(src, route_sel, topology, out, depth + 1);
        }
        return;
    }
    if let Some(mul) = topology.clock_multipliers.iter().find(|m| m.id == entity_id) {
        collect_clock_routes(mul.source_id, selector, topology, out, depth + 1);
    }
}

/// Finds the Feature Unit on the playback path downstream of `terminal_link` leading to an output
/// terminal (headphones / speaker / line out).
fn find_playback_feature_unit(terminal_link: u8, topology: &AudioTopology) -> Option<FeatureUnit> {
    // Walk upstream from every non-USB OutputTerminal (speaker/headphones: 0x0300..0x03FF)
    // and keep the first FeatureUnit whose upstream chain reaches `terminal_link`.
    let mut best: Option<FeatureUnit> = None;
    for ot in &topology.output_terminals {
        if (ot.terminal_type & 0xFF00) != 0x0300 {
            continue;
        }
        let mut found_fu: Option<FeatureUnit> = None;
        if reaches_terminal(ot.source_id, terminal_link, topology, &mut found_fu, 0)
            && let Some(fu) = found_fu
        {
            if fu.has_volume() {
                return Some(fu);
            }
            if best.is_none() {
                best = Some(fu);
            }
        }
    }
    // Fallback: any FeatureUnit directly sourcing `terminal_link`.
    best.or_else(|| {
        topology
            .feature_units
            .iter()
            .find(|fu| fu.source_id == terminal_link && (fu.has_volume() || fu.has_mute()))
            .cloned()
    })
}

fn reaches_terminal(
    entity_id: u8,
    target_terminal: u8,
    topology: &AudioTopology,
    found_fu: &mut Option<FeatureUnit>,
    depth: u8,
) -> bool {
    if depth > 12 {
        return false;
    }
    if entity_id == target_terminal {
        return true;
    }
    if let Some(fu) = topology.feature_units.iter().find(|f| f.unit_id == entity_id) {
        if reaches_terminal(fu.source_id, target_terminal, topology, found_fu, depth + 1) {
            if found_fu.is_none() || fu.has_volume() {
                *found_fu = Some(fu.clone());
            }
            return true;
        }
        return false;
    }
    if let Some((_, sources)) = topology.extra_sources.iter().find(|(id, _)| *id == entity_id) {
        for &src in sources {
            if reaches_terminal(src, target_terminal, topology, found_fu, depth + 1) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a realistic UAC2 dongle descriptor (like EPZ TP35 Pro / CS43131 / CS43198):
    /// - AC interface 0 (UAC2): Clock Source 41 (programmable), USB Input Terminal 1,
    ///   Feature Unit 10 (master mute + L/R volume), Headphones Output Terminal 2.
    /// - AS interface 1: alt 0 (idle), alt 1 (24-bit in 3-byte S24_3LE, async OUT 0x01 + FB IN 0x81),
    ///   alt 2 (32-bit in 4-byte S32_LE, async OUT 0x01 + FB IN 0x81),
    ///   alt 3 (16-bit in 2-byte S16_LE, async OUT 0x01 + FB IN 0x81).
    pub(crate) fn sample_uac2_dac_descriptors() -> Vec<u8> {
        let mut dev = vec![
            18, DESC_DEVICE, 0x00, 0x02, // bcdUSB 2.00
            0xEF, 0x02, 0x01, 64,        // IAD misc class, ep0 64B
            0x34, 0x12, 0x78, 0x56,      // VID 0x1234, PID 0x5678
            0x00, 0x01, 1, 2, 3, 1,      // bNumConfigurations = 1
        ];

        let mut cfg_body = Vec::new();
        // Interface 0, Alt 0: AudioControl UAC2
        cfg_body.extend_from_slice(&[9, DESC_INTERFACE, 0, 0, 0, CLASS_AUDIO, SUBCLASS_AUDIOCONTROL, PROTO_UAC2, 0]);
        // AC Header (UAC2)
        cfg_body.extend_from_slice(&[9, DESC_CS_INTERFACE, 0x01, 0x00, 0x02, 0x01, 64, 0, 0]);
        // Clock Source ID=41, internal programmable, SamFreq RW + ClockValid RO (0b0111 = 0x07)
        cfg_body.extend_from_slice(&[8, DESC_CS_INTERFACE, AC_CLOCK_SOURCE, 41, 0x03, 0x07, 0, 0]);
        // Input Terminal ID=1, USB Streaming (0x0101), bCSourceID=41, 2ch
        cfg_body.extend_from_slice(&[
            17, DESC_CS_INTERFACE, AC_INPUT_TERMINAL, 1, 0x01, 0x01, 0, 41, 2, 0x03, 0, 0, 0, 0, 0, 0, 0,
        ]);
        // Feature Unit ID=10, bSourceID=1, master mute RW (0x03), ch1 vol RW (0x0C), ch2 vol RW (0x0C)
        cfg_body.extend_from_slice(&[
            18, DESC_CS_INTERFACE, AC_FEATURE_UNIT, 10, 1,
            0x03, 0, 0, 0, // master: mute RW
            0x0C, 0, 0, 0, // ch1: volume RW
            0x0C, 0, 0, 0, // ch2: volume RW
            0,             // iFeature
        ]);
        // Output Terminal ID=2, Headphones (0x0302), bSourceID=10, bCSourceID=41
        cfg_body.extend_from_slice(&[12, DESC_CS_INTERFACE, AC_OUTPUT_TERMINAL, 2, 0x02, 0x03, 0, 10, 41, 0, 0, 0]);

        // Interface 1, Alt 0: AudioStreaming zero bandwidth
        cfg_body.extend_from_slice(&[9, DESC_INTERFACE, 1, 0, 0, CLASS_AUDIO, SUBCLASS_AUDIOSTREAMING, PROTO_UAC2, 0]);

        // Helper for UAC2 AS altsetting
        let push_alt = |buf: &mut Vec<u8>, alt: u8, subslot: u8, bits: u8, mps: u16| {
            buf.extend_from_slice(&[9, DESC_INTERFACE, 1, alt, 2, CLASS_AUDIO, SUBCLASS_AUDIOSTREAMING, PROTO_UAC2, 0]);
            // AS_GENERAL (16 bytes): terminal=1, controls=0, formatType=1, bmFormats=PCM(1), channels=2
            buf.extend_from_slice(&[
                16, DESC_CS_INTERFACE, AS_GENERAL, 1, 0, FORMAT_TYPE_I, 1, 0, 0, 0, 2, 3, 0, 0, 0, 0,
            ]);
            // FORMAT_TYPE I (6 bytes): subslot, bitResolution
            buf.extend_from_slice(&[6, DESC_CS_INTERFACE, AS_FORMAT_TYPE, FORMAT_TYPE_I, subslot, bits]);
            // Iso Async OUT EP 0x01, bInterval=1 (125us microframes)
            let mps_le = mps.to_le_bytes();
            buf.extend_from_slice(&[7, DESC_ENDPOINT, 0x01, 0x05, mps_le[0], mps_le[1], 1]);
            // CS_ENDPOINT EP_GENERAL
            buf.extend_from_slice(&[8, DESC_CS_ENDPOINT, EP_GENERAL, 0, 0, 0, 0, 0]);
            // Explicit Feedback IN EP 0x81 (usage=0b01 -> 0x11), mps=4, bInterval=4
            buf.extend_from_slice(&[7, DESC_ENDPOINT, 0x81, 0x11, 4, 0, 4]);
        };

        push_alt(&mut cfg_body, 1, 3, 24, 384); // 24-bit packed
        push_alt(&mut cfg_body, 2, 4, 32, 512); // 32-bit
        push_alt(&mut cfg_body, 3, 2, 16, 256); // 16-bit

        let total = (9 + cfg_body.len()) as u16;
        let total_le = total.to_le_bytes();
        dev.extend_from_slice(&[9, DESC_CONFIGURATION, total_le[0], total_le[1], 2, 1, 0, 0x80, 100]);
        dev.extend_from_slice(&cfg_body);
        dev
    }

    /// Builds a realistic UAC1 DAC descriptor (like DragonFly / UAC1 mode dongles).
    pub(crate) fn sample_uac1_dac_descriptors() -> Vec<u8> {
        let mut dev = vec![
            18, DESC_DEVICE, 0x10, 0x01, // bcdUSB 1.10
            0, 0, 0, 64,
            0xAC, 0x05, 0x01, 0x00,
            0x00, 0x01, 1, 2, 3, 1,
        ];
        let mut cfg_body = Vec::new();
        // AC Interface 0 (UAC1)
        cfg_body.extend_from_slice(&[9, DESC_INTERFACE, 0, 0, 0, CLASS_AUDIO, SUBCLASS_AUDIOCONTROL, 0, 0]);
        // AC Header
        cfg_body.extend_from_slice(&[9, DESC_CS_INTERFACE, 0x01, 0x00, 0x01, 30, 0, 1, 1]);
        // Input Terminal ID=1, USB Streaming
        cfg_body.extend_from_slice(&[12, DESC_CS_INTERFACE, AC_INPUT_TERMINAL, 1, 0x01, 0x01, 0, 2, 3, 0, 0, 0]);
        // Feature Unit ID=2, source=1, ctrlSize=1, master=mute+vol(0x03), ch1=0, ch2=0
        cfg_body.extend_from_slice(&[10, DESC_CS_INTERFACE, AC_FEATURE_UNIT, 2, 1, 1, 0x03, 0, 0, 0]);
        // Output Terminal ID=3, Headphones, source=2
        cfg_body.extend_from_slice(&[9, DESC_CS_INTERFACE, AC_OUTPUT_TERMINAL, 3, 0x02, 0x03, 0, 2, 0]);

        // AS Interface 1, Alt 0
        cfg_body.extend_from_slice(&[9, DESC_INTERFACE, 1, 0, 0, CLASS_AUDIO, SUBCLASS_AUDIOSTREAMING, 0, 0]);
        // AS Interface 1, Alt 1: 24-bit 3-byte, discrete 44.1 / 48 / 88.2 / 96 kHz
        cfg_body.extend_from_slice(&[9, DESC_INTERFACE, 1, 1, 1, CLASS_AUDIO, SUBCLASS_AUDIOSTREAMING, 0, 0]);
        cfg_body.extend_from_slice(&[7, DESC_CS_INTERFACE, AS_GENERAL, 1, 1, 0x01, 0x00]);
        let mut fmt = vec![8 + 4 * 3, DESC_CS_INTERFACE, AS_FORMAT_TYPE, FORMAT_TYPE_I, 2, 3, 24, 4];
        for r in [44_100u32, 48_000, 88_200, 96_000] {
            let b = r.to_le_bytes();
            fmt.extend_from_slice(&b[..3]);
        }
        cfg_body.extend_from_slice(&fmt);
        // Adaptive OUT EP 0x02, mps=582, interval=1, synchAddr=0
        let mps = 582u16.to_le_bytes();
        cfg_body.extend_from_slice(&[9, DESC_ENDPOINT, 0x02, 0x09, mps[0], mps[1], 1, 0, 0]);
        // CS_ENDPOINT EP_GENERAL with SAMPLING_FREQ_CONTROL bit 0 set
        cfg_body.extend_from_slice(&[7, DESC_CS_ENDPOINT, EP_GENERAL, 0x01, 0, 0, 0]);

        let total = (9 + cfg_body.len()) as u16;
        let total_le = total.to_le_bytes();
        dev.extend_from_slice(&[9, DESC_CONFIGURATION, total_le[0], total_le[1], 2, 1, 0, 0x80, 50]);
        dev.extend_from_slice(&cfg_body);
        dev
    }

    #[test]
    fn parses_uac2_dac_topology_clocks_feature_unit_and_feedback() {
        let raw = sample_uac2_dac_descriptors();
        let parsed = parse_raw_descriptors(&raw, None).unwrap();
        assert_eq!(parsed.vendor_id, 0x1234);
        assert_eq!(parsed.product_id, 0x5678);
        assert_eq!(parsed.uac_version, UacVersion::Uac2);
        assert_eq!(parsed.ac_interface, 0);
        assert_eq!(parsed.as_interface(), 1);
        assert_eq!(parsed.required_interfaces(), vec![0, 1]);
        assert_eq!(parsed.alt_settings.len(), 3);
        assert_eq!(parsed.alt_settings[0].subslot_bytes, 3);
        assert_eq!(parsed.alt_settings[0].bit_resolution, 24);
        assert_eq!(parsed.alt_settings[0].out_endpoint.sync_type, IsoSyncType::Asynchronous);
        assert_eq!(parsed.alt_settings[0].feedback_endpoint.unwrap().address, 0x81);
        assert_eq!(parsed.clock_routes.len(), 1);
        assert_eq!(parsed.clock_routes[0].clock_source.id, 41);
        assert!(parsed.clock_routes[0].clock_source.freq_programmable());
        let fu = parsed.feature_unit.unwrap();
        assert_eq!(fu.unit_id, 10);
        assert!(fu.master_mute);
        assert_eq!(fu.channel_volumes, vec![1, 2]);
    }

    #[test]
    fn parses_uac1_dac_with_discrete_rates_and_ep_freq_control() {
        let raw = sample_uac1_dac_descriptors();
        let parsed = parse_raw_descriptors(&raw, None).unwrap();
        assert_eq!(parsed.uac_version, UacVersion::Uac1);
        assert_eq!(parsed.alt_settings.len(), 1);
        let alt = &parsed.alt_settings[0];
        assert_eq!(alt.bit_resolution, 24);
        assert_eq!(alt.subslot_bytes, 3);
        assert!(alt.out_endpoint.has_sampling_freq_control);
        assert_eq!(alt.out_endpoint.sync_type, IsoSyncType::Adaptive);
        assert_eq!(
            alt.uac1_rates,
            Some(Uac1SampleRates::Discrete(vec![44_100, 48_000, 88_200, 96_000]))
        );
        let fu = parsed.feature_unit.unwrap();
        assert_eq!(fu.unit_id, 2);
        assert!(fu.master_mute);
        assert!(fu.master_volume);
    }

    #[test]
    fn parses_uac2_clock_selector_routes() {
        let mut raw = sample_uac2_dac_descriptors();
        // Build a compact UAC2 descriptor with Clock Selector 50 -> Clock Sources 41 & 42.
        raw.clear();
        raw.extend_from_slice(&[18, DESC_DEVICE, 0, 2, 0xEF, 2, 1, 64, 1, 0, 2, 0, 0, 1, 0, 0, 0, 1]);
        let mut cfg = Vec::new();
        cfg.extend_from_slice(&[9, DESC_INTERFACE, 0, 0, 0, CLASS_AUDIO, SUBCLASS_AUDIOCONTROL, PROTO_UAC2, 0]);
        cfg.extend_from_slice(&[8, DESC_CS_INTERFACE, AC_CLOCK_SOURCE, 41, 0x01, 0x01, 0, 0]); // fixed RO
        cfg.extend_from_slice(&[8, DESC_CS_INTERFACE, AC_CLOCK_SOURCE, 42, 0x03, 0x03, 0, 0]); // programmable RW
        cfg.extend_from_slice(&[8, DESC_CS_INTERFACE, AC_CLOCK_SELECTOR, 50, 2, 41, 42, 0x03]);
        cfg.extend_from_slice(&[
            17, DESC_CS_INTERFACE, AC_INPUT_TERMINAL, 1, 0x01, 0x01, 0, 50, 2, 3, 0, 0, 0, 0, 0, 0, 0,
        ]);
        cfg.extend_from_slice(&[9, DESC_INTERFACE, 1, 1, 1, CLASS_AUDIO, SUBCLASS_AUDIOSTREAMING, PROTO_UAC2, 0]);
        cfg.extend_from_slice(&[
            16, DESC_CS_INTERFACE, AS_GENERAL, 1, 0, FORMAT_TYPE_I, 1, 0, 0, 0, 2, 3, 0, 0, 0, 0,
        ]);
        cfg.extend_from_slice(&[6, DESC_CS_INTERFACE, AS_FORMAT_TYPE, FORMAT_TYPE_I, 4, 32]);
        cfg.extend_from_slice(&[7, DESC_ENDPOINT, 0x01, 0x05, 0, 2, 1]);
        let total = (9 + cfg.len()) as u16;
        raw.extend_from_slice(&[9, DESC_CONFIGURATION, total.to_le_bytes()[0], total.to_le_bytes()[1], 2, 1, 0, 0x80, 50]);
        raw.extend_from_slice(&cfg);

        let parsed = parse_raw_descriptors(&raw, None).unwrap();
        assert_eq!(parsed.clock_routes.len(), 2);
        // Programmable clock 42 is prioritized over fixed clock 41.
        assert_eq!(parsed.clock_routes[0].clock_source.id, 42);
        assert_eq!(parsed.clock_routes[0].selector, Some((50, 2)));
        assert_eq!(parsed.clock_routes[1].clock_source.id, 41);
        assert_eq!(parsed.clock_routes[1].selector, Some((50, 1)));
    }
}

//! Bit-exact PCM format selection and wire packing for USB Audio Class Type I streams.
//!
//! Per the UAC1 (Type I 2.2.2) and UAC2 (Formats 2.3.1.5) specifications, USB Audio Type I PCM
//! samples are ALWAYS little-endian signed two's complement, MSB-aligned (left-justified) inside
//! `bSubslotSize` bytes with unused LSBs zero-padded:
//! - 2-byte subslot: `S16_LE`
//! - 3-byte subslot: `S24_3LE` (16-bit input is shifted left by 8, LSB = 0x00)
//! - 4-byte subslot: `S32_LE` (16-bit shifted left by 16, 24-bit shifted left by 8 — 100% bit-exact)
//!
//! No dithering, no gain scaling, and no sample-rate conversion ever occur here.

use crate::descriptors::{StreamAltSetting, UacVersion};

/// Input PCM encoding delivered by the player/decoder (matches Android/Media3 `C.ENCODING_PCM_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPcmFormat {
    /// 16-bit signed little-endian integer.
    Pcm16Le,
    /// 24-bit signed little-endian packed (3 bytes/sample).
    Pcm24Le,
    /// 24-bit signed big-endian packed (3 bytes/sample).
    Pcm24Be,
    /// 32-bit signed little-endian integer (full 32-bit MSB-aligned).
    Pcm32Le,
    /// 32-bit signed big-endian integer.
    Pcm32Be,
    /// 32-bit IEEE-754 little-endian float (`[-1.0, 1.0)`).
    ///
    /// Android's `c2.android.flac.decoder` outputs 16-bit and 24-bit FLAC as `sample / 2^15` or
    /// `sample / 2^23` in IEEE-754 float32, which is a pure exponent shift (all 16 or 24 bits are
    /// preserved with zero rounding error and recovered bit-for-bit identically).
    PcmFloat32Le,
}

impl InputPcmFormat {
    /// Bytes per individual sample (one channel).
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::Pcm16Le => 2,
            Self::Pcm24Le | Self::Pcm24Be => 3,
            Self::Pcm32Le | Self::Pcm32Be | Self::PcmFloat32Le => 4,
        }
    }

    /// Significant bits carried by this PCM representation when `source_bits` is unknown (0).
    pub const fn default_bits(self) -> u8 {
        match self {
            Self::Pcm16Le => 16,
            Self::Pcm24Le | Self::Pcm24Be | Self::PcmFloat32Le => 24,
            Self::Pcm32Le | Self::Pcm32Be => 32,
        }
    }
}

/// Picks the best bit-perfect alternate setting for a track's exact sample rate, channel count,
/// and bit depth.
///
/// Rules:
/// - For UAC1, the altsetting MUST natively support `sample_rate` (zero resampling!).
/// - Prefer exact channel match (`channels == wanted_channels`, or stereo fallback for mono input
///   when the DAC only offers stereo — duplicating L -> L+R is lossless).
/// - Prefer `bit_resolution >= wanted_bits` (so hi-res audio is NEVER truncated when a wider
///   altsetting exists), and among those, prefer the tightest bit depth / subslot size first
///   (e.g., 16-bit track -> 16-bit if offered, else 24-bit zero-padded, else 32-bit zero-padded;
///   24-bit track -> 24-bit if offered, else 32-bit zero-padded).
/// - Prefer asynchronous endpoints with explicit feedback over adaptive/synchronous.
pub fn select_alt_setting(
    alts: &[StreamAltSetting],
    uac_version: UacVersion,
    sample_rate: u32,
    input_channels: u8,
    wanted_bits: u8,
) -> Option<&StreamAltSetting> {
    let target_bits = if wanted_bits > 0 { wanted_bits } else { 16 };
    alts.iter()
        .filter(|alt| {
            // Channel compatibility: exact match, or mono duplicated to stereo.
            let ch_ok = alt.channels == input_channels || (input_channels == 1 && alt.channels == 2);
            if !ch_ok {
                return false;
            }
            if uac_version == UacVersion::Uac1 {
                alt.uac1_rates
                    .as_ref()
                    .is_some_and(|r| r.supports(sample_rate))
            } else {
                true
            }
        })
        .min_by_key(|alt| {
        let ch_mismatch = u8::from(alt.channels != input_channels);
        // Strongly prefer not truncating bits (0 = preserves all source bits, 1 = truncates).
        let truncates = u8::from(alt.bit_resolution < target_bits);
        // Among non-truncating alts, prefer closest bit_resolution >= target_bits;
        // among truncating alts (if DAC only has 16-bit for a 24-bit file), prefer highest resolution.
        let bit_distance = if alt.bit_resolution >= target_bits {
            alt.bit_resolution - target_bits
        } else {
            u8::MAX - alt.bit_resolution
        };
        let subslot_overhead = alt.subslot_bytes.saturating_mul(8).saturating_sub(alt.bit_resolution);
        let no_feedback = u8::from(alt.feedback_endpoint.is_none());
        (ch_mismatch, truncates, bit_distance, subslot_overhead, no_feedback, alt.alt_setting)
    })
}

/// Packs input PCM bytes into USB Audio Type I wire bytes (little-endian, MSB-aligned in
/// `out_subslot_bytes` of 2, 3, or 4 bytes per sample).
///
/// Supports `in_channels == out_channels`, and mono (`1`) duplicated cleanly into stereo (`2`).
/// Appends packed bytes to `out` and returns the number of audio frames packed.
pub fn pack_pcm_to_wire(
    input: &[u8],
    in_format: InputPcmFormat,
    in_channels: u8,
    out_channels: u8,
    out_subslot_bytes: u8,
    out: &mut Vec<u8>,
) -> usize {
    let in_ch = in_channels as usize;
    let out_ch = out_channels as usize;
    let in_bps = in_format.bytes_per_sample();
    let in_frame_bytes = in_ch * in_bps;
    if in_frame_bytes == 0 || out_ch == 0 || !matches!(out_subslot_bytes, 2..=4) {
        return 0;
    }
    let frames = input.len() / in_frame_bytes;
    if frames == 0 {
        return 0;
    }
    let out_sub = out_subslot_bytes as usize;
    out.reserve(frames * out_ch * out_sub);

    // Fast exact-copy paths when wire format matches input format directly!
    if in_ch == out_ch {
        match (in_format, out_subslot_bytes) {
            (InputPcmFormat::Pcm16Le, 2)
            | (InputPcmFormat::Pcm24Le, 3)
            | (InputPcmFormat::Pcm32Le, 4) => {
                out.extend_from_slice(&input[..frames * in_frame_bytes]);
                return frames;
            }
            _ => {}
        }
    }

    let mono_to_stereo = in_ch == 1 && out_ch == 2;
    let channels_to_read = if mono_to_stereo { 1 } else { in_ch.min(out_ch) };

    for f in 0..frames {
        let frame_base = f * in_frame_bytes;
        for c in 0..channels_to_read {
            let s = read_sample_as_i32_msb(&input[frame_base + c * in_bps..], in_format);
            write_msb_sample(s, out_subslot_bytes, out);
            if mono_to_stereo {
                write_msb_sample(s, out_subslot_bytes, out);
            }
        }
        // If output has extra channels beyond input (not mono->stereo), zero-pad them.
        if !mono_to_stereo && out_ch > channels_to_read {
            for _ in channels_to_read..out_ch {
                write_msb_sample(0, out_subslot_bytes, out);
            }
        }
    }
    frames
}

/// Reads one sample from `src` and returns it as a signed 32-bit MSB-aligned integer (`i32`).
#[inline]
fn read_sample_as_i32_msb(src: &[u8], fmt: InputPcmFormat) -> i32 {
    match fmt {
        InputPcmFormat::Pcm16Le => {
            let s = i16::from_le_bytes([src[0], src[1]]);
            i32::from(s) << 16
        }
        InputPcmFormat::Pcm24Le => {
            // 3-byte signed LE -> bits 31..8, bits 7..0 = 0
            i32::from_le_bytes([0, src[0], src[1], src[2]])
        }
        InputPcmFormat::Pcm24Be => {
            // 3-byte signed BE -> bits 31..8, bits 7..0 = 0
            i32::from_le_bytes([0, src[2], src[1], src[0]])
        }
        InputPcmFormat::Pcm32Le => i32::from_le_bytes([src[0], src[1], src[2], src[3]]),
        InputPcmFormat::Pcm32Be => i32::from_be_bytes([src[0], src[1], src[2], src[3]]),
        InputPcmFormat::PcmFloat32Le => {
            let f = f32::from_le_bytes([src[0], src[1], src[2], src[3]]);
            if !f.is_finite() {
                return 0;
            }
            // Scale by 2^31 in f64: for any i16/2^15 or i24/2^23 value in f32, multiplying by
            // 2^31 is an exact power-of-2 exponent shift yielding `s16 << 16` or `s24 << 8` with
            // ZERO rounding error! Clamp handles +1.0f cleanly without wrapping to i32::MIN.
            let scaled = (f64::from(f) * 2_147_483_648.0).round();
            if scaled >= 2_147_483_647.0 {
                i32::MAX
            } else if scaled <= -2_147_483_648.0 {
                i32::MIN
            } else {
                scaled as i32
            }
        }
    }
}

/// Writes an MSB-aligned signed `i32` into 2, 3, or 4 little-endian wire bytes.
#[inline]
fn write_msb_sample(sample_msb: i32, subslot_bytes: u8, out: &mut Vec<u8>) {
    let le = sample_msb.to_le_bytes();
    match subslot_bytes {
        2 => out.extend_from_slice(&le[2..4]),
        3 => out.extend_from_slice(&le[1..4]),
        4 => out.extend_from_slice(&le[0..4]),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptors::parse_raw_descriptors;
    use crate::descriptors::tests::{sample_uac1_dac_descriptors, sample_uac2_dac_descriptors};

    #[test]
    fn exhaustive_i16_via_float32_is_bit_exact_for_16_24_32_wire() {
        let mut float_bytes = Vec::with_capacity(65536 * 4);
        for s in i16::MIN..=i16::MAX {
            let f = (s as f32) / 32768.0f32;
            float_bytes.extend_from_slice(&f.to_le_bytes());
        }

        let mut wire16 = Vec::new();
        assert_eq!(pack_pcm_to_wire(&float_bytes, InputPcmFormat::PcmFloat32Le, 1, 1, 2, &mut wire16), 65536);
        let mut wire24 = Vec::new();
        assert_eq!(pack_pcm_to_wire(&float_bytes, InputPcmFormat::PcmFloat32Le, 1, 1, 3, &mut wire24), 65536);
        let mut wire32 = Vec::new();
        assert_eq!(pack_pcm_to_wire(&float_bytes, InputPcmFormat::PcmFloat32Le, 1, 1, 4, &mut wire32), 65536);

        for (idx, s) in (i16::MIN..=i16::MAX).enumerate() {
            let got16 = i16::from_le_bytes([wire16[idx * 2], wire16[idx * 2 + 1]]);
            assert_eq!(got16, s, "S16 mismatch at {s}");

            let exp_le = (i32::from(s) << 16).to_le_bytes();
            assert_eq!(&wire24[idx * 3..idx * 3 + 3], &exp_le[1..4], "S24_3LE mismatch at {s}");
            assert_eq!(&wire32[idx * 4..idx * 4 + 4], &exp_le[0..4], "S32_LE mismatch at {s}");
        }
    }

    #[test]
    fn i24_boundaries_and_stride_via_float32_are_bit_exact() {
        let samples = [
            -8_388_608i32,
            -8_388_607,
            -4_194_304,
            -123_456,
            -1,
            0,
            1,
            123_456,
            4_194_304,
            8_388_606,
            8_388_607,
        ];
        let mut float_bytes = Vec::new();
        for s in samples {
            let f = (s as f32) / 8_388_608.0f32;
            float_bytes.extend_from_slice(&f.to_le_bytes());
        }
        let mut wire24 = Vec::new();
        pack_pcm_to_wire(&float_bytes, InputPcmFormat::PcmFloat32Le, 1, 1, 3, &mut wire24);
        let mut wire32 = Vec::new();
        pack_pcm_to_wire(&float_bytes, InputPcmFormat::PcmFloat32Le, 1, 1, 4, &mut wire32);

        for (i, &s) in samples.iter().enumerate() {
            let exp_msb = (s << 8).to_le_bytes();
            assert_eq!(&wire24[i * 3..i * 3 + 3], &exp_msb[1..4], "24-bit wire mismatch for {s}");
            assert_eq!(&wire32[i * 4..i * 4 + 4], &exp_msb[0..4], "32-bit wire mismatch for {s}");
        }
    }

    #[test]
    fn direct_integer_packing_preserves_every_bit_and_duplicates_mono() {
        // 24-bit packed LE input: [0x11, 0x22, 0x33] (0x332211)
        let in24 = [0x11u8, 0x22, 0x33];
        let mut out32_stereo = Vec::new();
        assert_eq!(pack_pcm_to_wire(&in24, InputPcmFormat::Pcm24Le, 1, 2, 4, &mut out32_stereo), 1);
        assert_eq!(out32_stereo, vec![0x00, 0x11, 0x22, 0x33, 0x00, 0x11, 0x22, 0x33]);

        // 32-bit LE passthrough
        let in32 = [0x78u8, 0x56, 0x34, 0x12, 0xF0, 0xDE, 0xBC, 0x9A];
        let mut out32 = Vec::new();
        assert_eq!(pack_pcm_to_wire(&in32, InputPcmFormat::Pcm32Le, 2, 2, 4, &mut out32), 1);
        assert_eq!(out32, in32);
    }

    #[test]
    fn alt_setting_selection_matches_bit_depth_and_uac1_rates() {
        let uac2 = parse_raw_descriptors(&sample_uac2_dac_descriptors(), None).unwrap();
        // 16-bit track picks 16-bit alt 3
        let a16 = select_alt_setting(&uac2.alt_settings, UacVersion::Uac2, 44_100, 2, 16).unwrap();
        assert_eq!((a16.alt_setting, a16.bit_resolution, a16.subslot_bytes), (3, 16, 2));
        // 24-bit track picks 24-bit alt 1
        let a24 = select_alt_setting(&uac2.alt_settings, UacVersion::Uac2, 192_000, 2, 24).unwrap();
        assert_eq!((a24.alt_setting, a24.bit_resolution, a24.subslot_bytes), (1, 24, 3));
        // 32-bit track picks 32-bit alt 2
        let a32 = select_alt_setting(&uac2.alt_settings, UacVersion::Uac2, 384_000, 2, 32).unwrap();
        assert_eq!((a32.alt_setting, a32.bit_resolution, a32.subslot_bytes), (2, 32, 4));

        let uac1 = parse_raw_descriptors(&sample_uac1_dac_descriptors(), None).unwrap();
        assert!(select_alt_setting(&uac1.alt_settings, UacVersion::Uac1, 96_000, 2, 24).is_some());
        // 192 kHz is NOT in the UAC1 discrete rate list -> rejected (no resampling!)
        assert!(select_alt_setting(&uac1.alt_settings, UacVersion::Uac1, 192_000, 2, 24).is_none());
    }
}

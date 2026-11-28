//! Isochronous packet pacing (16.16 fixed-point Bresenham accumulator) and UAC1/UAC2
//! asynchronous explicit-feedback rate decoding.

/// USB bus speed (determines 1 ms frames vs 125 µs microframes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    Full,
    High,
    Super,
}

impl UsbSpeed {
    /// Base isochronous service ticks per second (1000 for Full-Speed, 8000 for High/Super-Speed).
    pub const fn ticks_per_sec(self) -> u32 {
        match self {
            Self::Full => 1_000,
            Self::High | Self::Super => 8_000,
        }
    }

    pub const fn is_high_or_super(self) -> bool {
        matches!(self, Self::High | Self::Super)
    }
}

/// Computes how many isochronous packets per second an endpoint transfers given bus `speed` and
/// endpoint `bInterval` (1..=16 -> period `2^(bInterval - 1)` frames or microframes).
pub fn packets_per_sec(speed: UsbSpeed, b_interval: u8) -> u32 {
    let shift = b_interval.saturating_sub(1).min(15) as u32;
    let period = 1u32 << shift;
    (speed.ticks_per_sec() / period).max(1)
}

/// Fixed-point 16.16 packet-frame accumulator with validated asynchronous feedback tracking.
#[derive(Debug, Clone)]
pub struct IsoPacer {
    /// Nominal frames per OUT packet in 16.16 fixed point.
    nominal_fp16: u32,
    /// Active frames per OUT packet in 16.16 fixed point (tracks DAC feedback when async).
    current_fp16: u32,
    /// Fractional accumulator (`0..65536`).
    frac: u32,
    /// Upper bound on frames per packet so `frames * frame_bytes <= max_packet_size`.
    max_frames_per_packet: u32,
    /// `bInterval - 1` of the OUT endpoint (used when scaling High-Speed feedback).
    out_interval_shift: u32,
    /// Bus speed (Full-Speed = 10.14 3-byte feedback; High-Speed = 16.16 4-byte feedback).
    speed: UsbSpeed,
}

impl IsoPacer {
    pub fn new(
        sample_rate: u32,
        speed: UsbSpeed,
        out_b_interval: u8,
        max_packet_bytes: u16,
        frame_bytes: usize,
    ) -> Self {
        let pps = u64::from(packets_per_sec(speed, out_b_interval));
        let nominal_fp16 =
            ((((u64::from(sample_rate)) << 16) + pps / 2) / pps).min(u64::from(u32::MAX)) as u32;
        let max_frames = usize::from(max_packet_bytes)
            .checked_div(frame_bytes)
            .unwrap_or(1)
            .max(1) as u32;
        Self {
            nominal_fp16,
            current_fp16: nominal_fp16,
            frac: 0,
            max_frames_per_packet: max_frames,
            out_interval_shift: u32::from(out_b_interval.saturating_sub(1).min(7)),
            speed,
        }
    }

    #[cfg(test)]
    pub fn nominal_fp16(&self) -> u32 {
        self.nominal_fp16
    }

    #[cfg(test)]
    pub fn current_fp16(&self) -> u32 {
        self.current_fp16
    }

    /// Returns the exact integer number of audio frames to place in the next isochronous OUT packet.
    #[inline]
    pub fn next_packet_frames(&mut self) -> u32 {
        let total = u64::from(self.frac) + u64::from(self.current_fp16);
        let frames = ((total >> 16) as u32).min(self.max_frames_per_packet);
        self.frac = (total & 0xFFFF) as u32;
        frames
    }

    /// Consumes a raw explicit-feedback IN packet (3 bytes on Full-Speed, 3 or 4 bytes on
    /// High-Speed) and updates `current_fp16` if the value is within ±2% of the nominal rate.
    ///
    /// Handles common DAC firmware quirks (10.14 on High-Speed, pre-scaled vs microframe-scaled
    /// feedback) without ever letting garbage/startup zeros destabilize the clock.
    pub fn apply_feedback(&mut self, data: &[u8]) -> bool {
        let Some(candidate) = self.decode_feedback(data) else {
            return false;
        };
        // Smooth 50/50 IIR update to filter single-packet jitter while tracking crystal drift fast.
        self.current_fp16 = ((u64::from(self.current_fp16) + u64::from(candidate)) / 2) as u32;
        true
    }

    fn decode_feedback(&self, data: &[u8]) -> Option<u32> {
        let nom = u64::from(self.nominal_fp16);
        if nom == 0 {
            return None;
        }
        let tol = (nom / 50).max(64); // ±2% tolerance band
        let in_range = |v: u64| v >= nom.saturating_sub(tol) && v <= nom.saturating_add(tol);

        if data.len() >= 4 && self.speed.is_high_or_super() {
            let raw = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            // Standard UAC2 HS: 16.16 frames per 125us microframe -> scale by OUT interval.
            let scaled = u64::from(raw) << self.out_interval_shift;
            if in_range(scaled) {
                return Some(scaled as u32);
            }
            // Quirk 1: DAC already reports frames per OUT interval.
            if in_range(u64::from(raw)) {
                return Some(raw);
            }
            // Quirk 2: HS DAC erroneously encodes 10.14 in 4 bytes.
            let q10_14 = (u64::from(raw & 0x00FF_FFFF) << 2) << self.out_interval_shift;
            if in_range(q10_14) {
                return Some(q10_14 as u32);
            }
        }

        if data.len() >= 3 {
            let raw24 = u32::from_le_bytes([data[0], data[1], data[2], 0]);
            // Standard Full-Speed: 10.14 frames per 1ms frame -> convert to 16.16 (`<< 2`).
            let scaled = (u64::from(raw24) << 2) << self.out_interval_shift;
            if in_range(scaled) {
                return Some(scaled as u32);
            }
            let unscaled = u64::from(raw24) << 2;
            if in_range(unscaled) {
                return Some(unscaled as u32);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_44100_full_speed_and_high_speed_frame_totals() {
        // Full-Speed 1000 packets/sec -> 44,100 frames in 1000 packets (900x44 + 100x45).
        let mut p_fs = IsoPacer::new(44_100, UsbSpeed::Full, 1, 512, 6);
        let sum_fs: u32 = (0..1_000).map(|_| p_fs.next_packet_frames()).sum();
        assert!((sum_fs as i32 - 44_100).abs() <= 1, "sum_fs={sum_fs}");

        // High-Speed bInterval=1 -> 8000 microframes/sec -> 44,100 frames in 8000 packets.
        let mut p_hs = IsoPacer::new(44_100, UsbSpeed::High, 1, 512, 8);
        let sum_hs: u32 = (0..8_000).map(|_| p_hs.next_packet_frames()).sum();
        assert!((sum_hs as i32 - 44_100).abs() <= 1, "sum_hs={sum_hs}");

        // High-Speed 384 kHz -> exact 48 frames per microframe.
        let mut p_384 = IsoPacer::new(384_000, UsbSpeed::High, 1, 512, 8);
        for _ in 0..100 {
            assert_eq!(p_384.next_packet_frames(), 48);
        }
    }

    #[test]
    fn async_feedback_tracks_valid_clock_and_ignores_garbage() {
        // 48 kHz High-Speed bInterval=1 -> nominal 6.0 frames/microframe (6 << 16 = 0x0006_0000).
        let mut p = IsoPacer::new(48_000, UsbSpeed::High, 1, 512, 8);
        assert_eq!(p.nominal_fp16(), 6 << 16);

        // Zero / wild feedback is ignored.
        assert!(!p.apply_feedback(&[0, 0, 0, 0]));
        assert!(!p.apply_feedback(&0x0010_0000u32.to_le_bytes()));
        assert_eq!(p.current_fp16(), 6 << 16);

        // Slightly fast DAC crystal (+0.1%): accepted and smoothly tracked.
        let fast: u32 = (6 << 16) + 400;
        assert!(p.apply_feedback(&fast.to_le_bytes()));
        assert!(p.current_fp16() > (6 << 16) && p.current_fp16() <= fast);
    }

    #[test]
    fn full_speed_10_14_feedback_decodes_cleanly() {
        // 44.1 kHz Full-Speed -> 44.1 frames/ms in 10.14 = round(44.1 * 16384) = 722_534.
        let mut p = IsoPacer::new(44_100, UsbSpeed::Full, 1, 512, 6);
        let raw_10_14: u32 = 722_600;
        let bytes = raw_10_14.to_le_bytes();
        assert!(p.apply_feedback(&bytes[..3]));
        assert!(p.current_fp16() > p.nominal_fp16());
    }
}

//! Pure-Rust ITU-R BS.1770-4 / EBU R128 loudness & sample-peak analyzer for ReplayGain 2.0.
//!
//! Computes integrated loudness (LUFS) with 400 ms overlapping blocks (100 ms hop), dual-stage
//! gating (-70 LKFS absolute gate and -10 LU relative gate), and ReplayGain 2.0 target loudness
//! (-18.0 LUFS), plus pooled album gain across all tracks of an album.

use std::f64::consts::PI;

/// ReplayGain 2.0 reference loudness in LUFS (matches `rsgain` / `loudgain` / `foobar2000`).
pub const REPLAYGAIN_TARGET_LUFS: f64 = -18.0;

/// Absolute gating threshold in LKFS (ITU-R BS.1770-4).
const ABSOLUTE_GATE_LKFS: f64 = -70.0;
/// Relative gating offset in LU below the ungated-above-absolute mean.
const RELATIVE_GATE_LU: f64 = -10.0;

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

#[inline(always)]
fn decode_s16le(b: &[u8]) -> f64 {
    f64::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0
}

#[inline(always)]
fn decode_s24le(b: &[u8]) -> f64 {
    // Sign-extend the packed 24-bit sample through the top of an i32.
    f64::from(i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) / 8_388_608.0
}

#[inline(always)]
fn decode_s32le(b: &[u8]) -> f64 {
    f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])) / 2_147_483_648.0
}

#[inline(always)]
fn decode_f32le(b: &[u8]) -> f64 {
    f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Computes the two ITU-R BS.1770-4 K-weighting biquad stages for `sample_rate` Hz using the
/// bilinear transform (exact match to ITU-R BS.1770-4 Table 1 at 48 kHz and `libebur128` at all
/// other sample rates).
fn k_weighting_filters(sample_rate: u32) -> (Biquad, Biquad) {
    let fs = f64::from(sample_rate.max(8_000));

    // Stage 1: Acoustic head-effect high-shelf (+4 dB above ~1681.97 Hz, Q = 0.7071752369554196).
    let f0_pre = 1681.974450955533;
    let g_pre = 3.999843853973347;
    let q_pre = 0.7071752369554196;
    let k = (PI * f0_pre / fs).tan();
    let vh = 10f64.powf(g_pre / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q_pre + k * k;
    let pre = Biquad::new(
        (vh + vb * k / q_pre + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q_pre + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q_pre + k * k) / a0,
    );

    // Stage 2: Revised Low-frequency B-curve (RLB) high-pass (~38.135 Hz, Q = 0.5003270373238773).
    let f0_rlb = 38.13547087602444;
    let q_rlb = 0.5003270373238773;
    let kr = (PI * f0_rlb / fs).tan();
    let ar0 = 1.0 + kr / q_rlb + kr * kr;
    let rlb = Biquad::new(
        1.0,
        -2.0,
        1.0,
        2.0 * (kr * kr - 1.0) / ar0,
        (1.0 - kr / q_rlb + kr * kr) / ar0,
    );

    (pre, rlb)
}

/// Channel weighting factor `G_i` per ITU-R BS.1770-4 (1.0 for L/R/C, 0.0 for LFE, 1.41 for Ls/Rs).
fn channel_weight(ch: usize, total_channels: usize) -> f64 {
    if total_channels <= 3 {
        1.0
    } else if total_channels >= 6 {
        match ch {
            0..=2 => 1.0,
            3 => 0.0, // LFE
            4 | 5 => 1.41,
            _ => 1.0,
        }
    } else if total_channels == 4 || total_channels == 5 {
        match ch {
            0..=2 => 1.0,
            _ => 1.41,
        }
    } else {
        1.0
    }
}

/// Result of analyzing one audio track with EBU R128.
#[derive(Debug, Clone)]
pub struct R128TrackResult {
    pub gain_db: f32,
    pub peak: f32,
    pub integrated_lufs: f32,
    /// Per-400ms-block weighted mean-square energies (retained for exact pooled album gating).
    pub block_energies: Vec<f64>,
}

/// Streaming EBU R128 loudness & peak analyzer for a single track.
pub struct R128Analyzer {
    channels: usize,
    filters: Vec<(Biquad, Biquad)>,
    weights: Vec<f64>,
    hop_frames: usize,
    frames_in_hop: usize,
    cur_hop_energy: f64,
    /// Last 4 consecutive 100 ms hop energies forming a sliding 400 ms block.
    hop_ring: [f64; 4],
    hop_count: usize,
    block_energies: Vec<f64>,
    /// Fallback energy accumulator for tracks shorter than 400 ms.
    total_energy: f64,
    total_frames: u64,
    peak: f64,
}

impl R128Analyzer {
    pub fn new(sample_rate: u32, channels: u8) -> Self {
        let ch = usize::from(channels.clamp(1, 8));
        let (pre, rlb) = k_weighting_filters(sample_rate);
        let filters = vec![(pre, rlb); ch];
        let weights = (0..ch).map(|c| channel_weight(c, ch)).collect();
        // 100 ms hop -> 4 hops = 400 ms gating block (75% overlap).
        let hop_frames = ((u64::from(sample_rate.max(8_000)) + 5) / 10).max(1) as usize;
        Self {
            channels: ch,
            filters,
            weights,
            hop_frames,
            frames_in_hop: 0,
            cur_hop_energy: 0.0,
            hop_ring: [0.0; 4],
            hop_count: 0,
            block_energies: Vec::new(),
            total_energy: 0.0,
            total_frames: 0,
            peak: 0.0,
        }
    }

    /// Feeds interleaved signed 16-bit little-endian PCM bytes (`S16_LE`).
    pub fn feed_s16le(&mut self, bytes: &[u8]) {
        let fast = self.channels == 2;
        self.feed_pcm::<2>(bytes, fast, decode_s16le);
    }

    /// Feeds interleaved signed 24-bit little-endian packed PCM bytes (`S24_3LE`, 3 bytes/sample).
    pub fn feed_s24le(&mut self, bytes: &[u8]) {
        let fast = self.channels == 2;
        self.feed_pcm::<3>(bytes, fast, decode_s24le);
    }

    /// Feeds interleaved signed 32-bit little-endian PCM bytes (`S32_LE`).
    pub fn feed_s32le(&mut self, bytes: &[u8]) {
        let fast = self.channels == 2;
        self.feed_pcm::<4>(bytes, fast, decode_s32le);
    }

    /// Feeds interleaved IEEE-754 32-bit little-endian float PCM bytes (`F32_LE`). Non-finite
    /// samples (NaN / ±Inf) are ignored.
    pub fn feed_f32le(&mut self, bytes: &[u8]) {
        let fast = self.channels == 2;
        self.feed_pcm::<4>(bytes, fast, decode_f32le);
    }

    /// Shared interleaved-PCM loop. `decode` maps one `BPS`-byte sample to `[-1.0, 1.0)`; a
    /// non-finite result skips that sample. `stereo_fast` selects the 2-channel path that keeps
    /// both channels' biquad state and the peak in locals instead of indexing `self.filters`.
    #[inline(always)]
    fn feed_pcm<const BPS: usize>(&mut self, bytes: &[u8], stereo_fast: bool, decode: fn(&[u8]) -> f64) {
        if stereo_fast && self.channels == 2 {
            let (mut l_pre, mut l_rlb) = self.filters[0];
            let (mut r_pre, mut r_rlb) = self.filters[1];
            let (wl, wr) = (self.weights[0], self.weights[1]);
            let mut peak = self.peak;
            for frame in bytes.chunks_exact(2 * BPS) {
                let mut frame_energy = 0.0;
                let xl = decode(&frame[..BPS]);
                if xl.is_finite() {
                    peak = peak.max(xl.abs());
                    let y = l_rlb.process(l_pre.process(xl));
                    frame_energy += wl * y * y;
                }
                let xr = decode(&frame[BPS..]);
                if xr.is_finite() {
                    peak = peak.max(xr.abs());
                    let y = r_rlb.process(r_pre.process(xr));
                    frame_energy += wr * y * y;
                }
                self.push_frame_energy(frame_energy);
            }
            self.filters[0] = (l_pre, l_rlb);
            self.filters[1] = (r_pre, r_rlb);
            self.peak = peak;
            return;
        }

        let frame_bytes = self.channels * BPS;
        if frame_bytes == 0 {
            return;
        }
        for frame in bytes.chunks_exact(frame_bytes) {
            let mut frame_energy = 0.0;
            for (c, sample) in frame.as_chunks::<BPS>().0.iter().enumerate() {
                let x = decode(sample);
                if !x.is_finite() {
                    continue;
                }
                let abs_x = x.abs();
                if abs_x > self.peak {
                    self.peak = abs_x;
                }
                let (pre, rlb) = &mut self.filters[c];
                let y = rlb.process(pre.process(x));
                frame_energy += self.weights[c] * y * y;
            }
            self.push_frame_energy(frame_energy);
        }
    }

    #[inline]
    fn push_frame_energy(&mut self, frame_energy: f64) {
        self.cur_hop_energy += frame_energy;
        self.total_energy += frame_energy;
        self.total_frames += 1;
        self.frames_in_hop += 1;
        if self.frames_in_hop >= self.hop_frames {
            let mean_hop = self.cur_hop_energy / (self.hop_frames as f64);
            self.hop_ring[self.hop_count % 4] = mean_hop;
            self.hop_count += 1;
            self.cur_hop_energy = 0.0;
            self.frames_in_hop = 0;
            if self.hop_count >= 4 {
                let block_mean = 0.25 * (self.hop_ring[0] + self.hop_ring[1] + self.hop_ring[2] + self.hop_ring[3]);
                self.block_energies.push(block_mean);
            }
        }
    }

    /// Finalizes integrated loudness, ReplayGain 2.0 track gain, and sample peak.
    pub fn finish(mut self) -> R128TrackResult {
        if self.block_energies.is_empty() && self.total_frames > 0 {
            self.block_energies.push(self.total_energy / (self.total_frames as f64));
        }
        let lufs = integrated_loudness_lufs(&self.block_energies);
        let gain_db = lufs
            .map(|l| (REPLAYGAIN_TARGET_LUFS - l).clamp(-40.0, 24.0))
            .unwrap_or(0.0);
        R128TrackResult {
            gain_db: gain_db as f32,
            peak: self.peak.clamp(0.0, 10.0) as f32,
            integrated_lufs: lufs.unwrap_or(-70.0) as f32,
            block_energies: self.block_energies,
        }
    }
}

/// Computes exact EBU R128 integrated loudness (LUFS) from 400 ms block mean-square energies.
pub fn integrated_loudness_lufs(blocks: &[f64]) -> Option<f64> {
    gated_loudness_lufs(blocks.iter().copied())
}

/// Dual-gated integrated loudness over any re-iterable sequence of block energies (two streaming
/// passes over `blocks`; nothing is collected).
fn gated_loudness_lufs<I: Iterator<Item = f64> + Clone>(blocks: I) -> Option<f64> {
    // Absolute gate threshold in linear mean-square power: 10^((-70 + 0.691) / 10).
    let abs_thresh = 10f64.powf((ABSOLUTE_GATE_LKFS + 0.691) / 10.0);
    let mut sum1 = 0.0;
    let mut cnt1 = 0usize;
    for e in blocks.clone() {
        if e > abs_thresh {
            sum1 += e;
            cnt1 += 1;
        }
    }
    if cnt1 == 0 {
        return None;
    }
    let gamma_a = -0.691 + 10.0 * (sum1 / (cnt1 as f64)).log10();
    let rel_thresh = 10f64.powf((gamma_a + RELATIVE_GATE_LU + 0.691) / 10.0);

    let mut sum2 = 0.0;
    let mut cnt2 = 0usize;
    for e in blocks {
        if e > abs_thresh && e > rel_thresh {
            sum2 += e;
            cnt2 += 1;
        }
    }
    if cnt2 == 0 {
        return None;
    }
    let lufs = -0.691 + 10.0 * (sum2 / (cnt2 as f64)).log10();
    lufs.is_finite().then_some(lufs)
}

/// Computes pooled EBU R128 album gain and peak across multiple analyzed tracks.
pub fn compute_album_replay_gain(tracks: &[R128TrackResult]) -> (f32, f32) {
    compute_album_replay_gain_iter(tracks)
}

/// [`compute_album_replay_gain`] over borrowed tracks: pools every track's block energies in two
/// streaming passes without cloning tracks or concatenating their blocks.
pub fn compute_album_replay_gain_iter<'a, I>(tracks: I) -> (f32, f32)
where
    I: IntoIterator<Item = &'a R128TrackResult>,
    I::IntoIter: Clone,
{
    let tracks = tracks.into_iter();
    let max_peak = tracks.clone().map(|t| t.peak).fold(0.0f32, f32::max);
    let blocks = tracks.flat_map(|t| t.block_energies.iter().copied());
    let gain_db = gated_loudness_lufs(blocks)
        .map(|l| (REPLAYGAIN_TARGET_LUFS - l).clamp(-40.0, 24.0) as f32)
        .unwrap_or(0.0);
    (gain_db, max_peak)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_s16le(sample_rate: u32, freq_hz: f64, amplitude: f64, seconds: f64) -> Vec<u8> {
        let frames = (f64::from(sample_rate) * seconds) as usize;
        let mut out = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let v = amplitude * (2.0 * PI * freq_hz * (i as f64) / f64::from(sample_rate)).sin();
            let s = (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
            out.extend_from_slice(&s.to_le_bytes());
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn sine_wave_loudness_matches_ebu_r128_across_sample_rates() {
        // Stereo 997 Hz sine at amplitude 0.5 (-6.02 dBFS peak -> ~-9.03 dBFS RMS per ch -> ~-6.02 LKFS stereo).
        // ReplayGain 2.0 target is -18.0 LUFS, so gain should be ~ -12.0 dB across 44.1, 48, and 96 kHz.
        for sr in [44_100u32, 48_000, 96_000] {
            let pcm = sine_s16le(sr, 997.0, 0.5, 2.0);
            let mut analyzer = R128Analyzer::new(sr, 2);
            analyzer.feed_s16le(&pcm);
            let res = analyzer.finish();
            assert!(
                (res.peak - 0.5).abs() < 0.01,
                "sr={sr} peak={}",
                res.peak
            );
            assert!(
                (res.integrated_lufs - (-6.0)).abs() < 0.6,
                "sr={sr} lufs={}",
                res.integrated_lufs
            );
            assert!(
                (res.gain_db - (-12.0)).abs() < 0.6,
                "sr={sr} gain_db={}",
                res.gain_db
            );
        }
    }

    #[test]
    fn silence_returns_zero_gain_and_album_pooling_works() {
        let mut silent = R128Analyzer::new(48_000, 2);
        silent.feed_s16le(&vec![0u8; 48_000 * 4]);
        let s_res = silent.finish();
        assert_eq!(s_res.gain_db, 0.0);
        assert_eq!(s_res.peak, 0.0);

        let mut loud = R128Analyzer::new(48_000, 2);
        loud.feed_s16le(&sine_s16le(48_000, 997.0, 0.5, 1.5));
        let l_res = loud.finish();

        let mut quiet = R128Analyzer::new(48_000, 2);
        quiet.feed_s16le(&sine_s16le(48_000, 997.0, 0.125, 1.5));
        let q_res = quiet.finish();

        let (alb_gain, alb_peak) = compute_album_replay_gain(&[l_res.clone(), q_res.clone()]);
        assert!((alb_peak - l_res.peak).abs() < 0.01);
        assert!(alb_gain > l_res.gain_db && alb_gain < q_res.gain_db);

        // The borrowed-iterator form pools identically to a contiguous copy of all blocks.
        let (iter_gain, iter_peak) = compute_album_replay_gain_iter([&l_res, &q_res]);
        assert_eq!((iter_gain, iter_peak), (alb_gain, alb_peak));
        let pooled: Vec<f64> = [&l_res, &q_res].iter().flat_map(|t| t.block_energies.clone()).collect();
        let expected = (REPLAYGAIN_TARGET_LUFS - integrated_loudness_lufs(&pooled).unwrap()).clamp(-40.0, 24.0);
        assert!((f64::from(alb_gain) - expected).abs() < 1e-4);
    }

    /// Stereo sine as normalized f64 samples (L == R), for re-encoding into each PCM format.
    fn sine_samples(sample_rate: u32, amplitude: f64, seconds: f64) -> Vec<f64> {
        let frames = (f64::from(sample_rate) * seconds) as usize;
        (0..frames)
            .flat_map(|i| {
                let v = amplitude * (2.0 * PI * 997.0 * (i as f64) / f64::from(sample_rate)).sin();
                [v, v]
            })
            .collect()
    }

    fn analyze(channels: u8, feed: impl FnOnce(&mut R128Analyzer)) -> R128TrackResult {
        let mut a = R128Analyzer::new(48_000, channels);
        feed(&mut a);
        a.finish()
    }

    fn assert_close(a: &R128TrackResult, b: &R128TrackResult, tol: f32, what: &str) {
        assert!((a.integrated_lufs - b.integrated_lufs).abs() < tol, "{what}: lufs {} vs {}", a.integrated_lufs, b.integrated_lufs);
        assert!((a.gain_db - b.gain_db).abs() < tol, "{what}: gain {} vs {}", a.gain_db, b.gain_db);
        assert!((a.peak - b.peak).abs() < 0.001, "{what}: peak {} vs {}", a.peak, b.peak);
    }

    #[test]
    fn all_pcm_formats_agree_on_loudness_and_peak() {
        let samples = sine_samples(48_000, 0.5, 1.5);
        let s16: Vec<u8> = samples.iter().flat_map(|v| ((v * 32_767.0).round() as i16).to_le_bytes()).collect();
        let s24: Vec<u8> = samples
            .iter()
            .flat_map(|v| {
                let b = ((v * 8_388_607.0).round() as i32).to_le_bytes();
                [b[0], b[1], b[2]]
            })
            .collect();
        let s32: Vec<u8> = samples.iter().flat_map(|v| ((v * 2_147_483_647.0).round() as i32).to_le_bytes()).collect();
        let f32le: Vec<u8> = samples.iter().flat_map(|v| (*v as f32).to_le_bytes()).collect();

        let r16 = analyze(2, |a| a.feed_s16le(&s16));
        let r24 = analyze(2, |a| a.feed_s24le(&s24));
        let r32 = analyze(2, |a| a.feed_s32le(&s32));
        let rf = analyze(2, |a| a.feed_f32le(&f32le));
        assert!((r16.integrated_lufs - (-6.0)).abs() < 0.6, "lufs={}", r16.integrated_lufs);
        assert_close(&r16, &r24, 0.01, "s24le");
        assert_close(&r16, &r32, 0.01, "s32le");
        assert_close(&r16, &rf, 0.01, "f32le");
    }

    #[test]
    fn s24le_sign_extends_negative_samples() {
        // -0.5 full scale (0xC00000) must read as a negative half-scale peak, not +0.75.
        let neg_half = (-4_194_304i32).to_le_bytes();
        let r = analyze(1, |a| a.feed_s24le(&[neg_half[0], neg_half[1], neg_half[2]]));
        assert!((r.peak - 0.5).abs() < 1e-6, "peak={}", r.peak);
    }

    #[test]
    fn stereo_fast_path_matches_general_path() {
        let samples = sine_samples(48_000, 0.3, 1.2);
        let s16: Vec<u8> = samples.iter().flat_map(|v| ((v * 32_767.0).round() as i16).to_le_bytes()).collect();
        let f32le: Vec<u8> = samples.iter().flat_map(|v| (*v as f32).to_le_bytes()).collect();

        let fast16 = analyze(2, |a| a.feed_pcm::<2>(&s16, true, decode_s16le));
        let slow16 = analyze(2, |a| a.feed_pcm::<2>(&s16, false, decode_s16le));
        assert_eq!(fast16.block_energies, slow16.block_energies);
        assert_eq!((fast16.peak, fast16.gain_db), (slow16.peak, slow16.gain_db));

        // Split feeds carry filter state across calls identically on both paths.
        let mut fast = R128Analyzer::new(48_000, 2);
        let mut slow = R128Analyzer::new(48_000, 2);
        for chunk in f32le.chunks(4_096 * 8) {
            fast.feed_pcm::<4>(chunk, true, decode_f32le);
            slow.feed_pcm::<4>(chunk, false, decode_f32le);
        }
        assert_eq!(fast.finish().block_energies, slow.finish().block_energies);
    }

    #[test]
    fn non_finite_float_samples_are_ignored() {
        let samples = sine_samples(48_000, 0.5, 1.0);
        let clean: Vec<u8> = samples.iter().flat_map(|v| (*v as f32).to_le_bytes()).collect();
        let mut dirty = clean.clone();
        for (i, bad) in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY].into_iter().cycle().take(30).enumerate() {
            let at = (i * 997 + 13) * 4;
            dirty[at..at + 4].copy_from_slice(&bad.to_le_bytes());
        }
        let r_clean = analyze(2, |a| a.feed_f32le(&clean));
        let r_dirty = analyze(2, |a| a.feed_f32le(&dirty));
        assert!(r_dirty.peak.is_finite() && r_dirty.gain_db.is_finite() && r_dirty.integrated_lufs.is_finite());
        assert!(r_dirty.peak <= 0.5 + 1e-3, "peak={}", r_dirty.peak);
        assert_close(&r_clean, &r_dirty, 0.1, "nan/inf");

        // A buffer of only non-finite samples behaves like silence.
        let junk: Vec<u8> = std::iter::repeat_n(f32::NAN.to_le_bytes(), 48_000).flatten().collect();
        let r_junk = analyze(2, |a| a.feed_f32le(&junk));
        assert_eq!((r_junk.gain_db, r_junk.peak), (0.0, 0.0));
    }

    #[test]
    fn short_track_under_400ms_uses_whole_track_fallback() {
        let pcm = sine_s16le(48_000, 997.0, 0.5, 0.2);
        let r = analyze(2, |a| a.feed_s16le(&pcm));
        assert_eq!(r.block_energies.len(), 1, "one fallback block for a 200 ms track");
        assert!((r.integrated_lufs - (-6.0)).abs() < 1.0, "lufs={}", r.integrated_lufs);
        assert!((r.gain_db - (-12.0)).abs() < 1.0, "gain={}", r.gain_db);

        // Partial frames at the end of a buffer are dropped, never read out of bounds.
        let r_odd = analyze(2, |a| a.feed_s16le(&pcm[..pcm.len() - 3]));
        assert!(r_odd.integrated_lufs.is_finite());
    }
}

//! Pure audio-effect maths shared with Kotlin: equalizer presets/plan and ReplayGain factor.
//! Presets, band frequencies, the ±12 dB range and the pre-amp rule match the Linux build
//! (`rustygrade-audio/src/mpv_engine.rs`), so settings stay interchangeable.

use crate::dto::TrackInfo;

pub const EQ_BAND_HZ: [i32; 10] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
pub const EQ_MAX_GAIN_DB: f64 = 12.0;

const PRESETS: [(&str, [f64; 10]); 6] = [
    ("FLAT", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("BASS", [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("TREBLE", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0]),
    ("VOCAL", [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 1.0, 0.0, -1.0]),
    ("V-SHAPE", [5.0, 4.0, 2.0, 0.0, -2.0, -2.0, 0.0, 2.0, 4.0, 5.0]),
    ("WARM", [3.0, 3.0, 2.0, 1.0, 0.0, -1.0, -1.0, -2.0, -2.0, -3.0]),
];

#[derive(Debug, Clone, uniffi::Record)]
pub struct EqPreset {
    pub name: String,
    pub gains_db: Vec<f64>,
}

/// What the effect has to apply: one gain per band plus the input pre-amp.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EqPlan {
    pub band_gains_db: Vec<f32>,
    /// Negative: the largest boost, so boosted bands cannot clip.
    pub preamp_db: f32,
}

#[uniffi::export]
pub fn eq_presets() -> Vec<EqPreset> {
    PRESETS
        .iter()
        .map(|(name, gains)| EqPreset { name: (*name).to_owned(), gains_db: gains.to_vec() })
        .collect()
}

/// Gains of a named preset (case-insensitive); `None` for `CUSTOM` and unknown names.
#[uniffi::export]
pub fn eq_preset_gains(name: String) -> Option<Vec<f64>> {
    PRESETS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
        .map(|(_, g)| g.to_vec())
}

/// The effect settings for the stored gains. Disabled (or `bit_perfect`) means flat with no pre-amp.
#[uniffi::export]
pub fn eq_plan(enabled: bool, bit_perfect: bool, gains_db: Vec<f64>) -> EqPlan {
    if !enabled || bit_perfect {
        return EqPlan { band_gains_db: vec![0.0; EQ_BAND_HZ.len()], preamp_db: 0.0 };
    }
    let bands: Vec<f64> = (0..EQ_BAND_HZ.len())
        .map(|i| {
            let g = gains_db.get(i).copied().unwrap_or(0.0);
            if g.is_finite() { g.clamp(-EQ_MAX_GAIN_DB, EQ_MAX_GAIN_DB) } else { 0.0 }
        })
        .collect();
    let max_boost = bands.iter().copied().fold(0.0_f64, f64::max);
    EqPlan {
        band_gains_db: bands.into_iter().map(|g| g as f32).collect(),
        preamp_db: (-max_boost) as f32,
    }
}

/// Upper edge of each band in Hz: the geometric midpoint between neighbouring centres (the last
/// band reaches 20 kHz). `DynamicsProcessing` bands are defined by their cutoff.
#[uniffi::export]
pub fn eq_band_cutoffs() -> Vec<f32> {
    let mut edges: Vec<f32> = EQ_BAND_HZ
        .windows(2)
        .map(|w| ((w[0] as f64 * w[1] as f64).sqrt()) as f32)
        .collect();
    edges.push(20_000.0);
    edges
}

/// Gains of the two players at `t` (0..1) through an equal-power crossfade: the same curve the
/// desktop build uses, so the fade sounds identical.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CrossfadeGains {
    pub fade_out: f64,
    pub fade_in: f64,
}

#[uniffi::export]
pub fn crossfade_gains(t: f64) -> CrossfadeGains {
    let (fade_out, fade_in) = rustygrade_core::CrossfadeMath::equal_power_gains(t, 1.0);
    CrossfadeGains { fade_out, fade_in }
}

/// Linear gain for the track under `mode` (`"no"`, `"track"`, `"album"`), peak-limited.
#[uniffi::export]
pub fn replay_gain_factor(mode: String, track: TrackInfo) -> f32 {
    track.replay_gain().factor(&mode)
}

/// Peak-limited ReplayGain offset in dB for the track under `mode` (`"no"`, `"track"`, `"album"`).
/// Used to apply ReplayGain through the USB DAC's hardware Feature Unit in bit-perfect mode.
#[uniffi::export]
pub fn replay_gain_db(mode: String, track: TrackInfo) -> f32 {
    track.replay_gain().gain_db(&mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_the_desktop_build() {
        let p = eq_presets();
        assert_eq!(p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["FLAT", "BASS", "TREBLE", "VOCAL", "V-SHAPE", "WARM"]);
        assert!(p.iter().all(|p| p.gains_db.len() == EQ_BAND_HZ.len()));
        assert_eq!(eq_preset_gains("bass".into()).unwrap()[0], 6.0);
        assert!(eq_preset_gains("CUSTOM".into()).is_none());
    }

    #[test]
    fn plan_preamp_offsets_the_largest_boost_and_clamps() {
        let plan = eq_plan(true, false, vec![6.0, 5.0, -3.0, 20.0, f64::NAN]);
        assert_eq!(plan.band_gains_db.len(), 10);
        assert_eq!(plan.band_gains_db[3], 12.0, "clamped to +12 dB");
        assert_eq!(plan.band_gains_db[4], 0.0, "NaN becomes flat");
        assert_eq!(plan.preamp_db, -12.0);
        let cut_only = eq_plan(true, false, vec![-4.0; 10]);
        assert_eq!(cut_only.preamp_db, 0.0, "no boost, no pre-amp");
    }

    #[test]
    fn crossfade_gains_start_on_the_outgoing_track_and_end_on_the_incoming_one() {
        let start = crossfade_gains(0.0);
        let end = crossfade_gains(1.0);
        assert!((start.fade_out - 1.0).abs() < 1e-9 && start.fade_in.abs() < 1e-9);
        assert!(end.fade_out.abs() < 1e-3 && (end.fade_in - 1.0).abs() < 1e-9);
        let mid = crossfade_gains(0.5);
        assert!(mid.fade_out > 0.5 && mid.fade_in > 0.5, "equal power keeps the middle loud: {mid:?}");
    }

    #[test]
    fn disabled_or_bit_perfect_is_flat() {
        let flat = EqPlan { band_gains_db: vec![0.0; 10], preamp_db: 0.0 };
        assert_eq!(eq_plan(false, false, vec![6.0; 10]), flat);
        assert_eq!(eq_plan(true, true, vec![6.0; 10]), flat);
    }

    #[test]
    fn band_cutoffs_are_increasing_geometric_midpoints() {
        let c = eq_band_cutoffs();
        assert_eq!(c.len(), 10);
        assert!(c.windows(2).all(|w| w[0] < w[1]));
        assert!((c[0] - (32.0f32 * 64.0).sqrt()).abs() < 1e-3);
        assert_eq!(c[9], 20_000.0);
    }
}

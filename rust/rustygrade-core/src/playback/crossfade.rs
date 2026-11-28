use std::f64::consts::FRAC_PI_2;

use super::RepeatMode;
use crate::Track;

pub struct CrossfadeMath;

impl CrossfadeMath {
    /// C# `CrossfadeApplies` for two different queue slots.
    pub fn should_crossfade(
        crossfade_seconds: i32,
        repeat: RepeatMode,
        is_direct: bool,
        shuffle: bool,
        current: &Track,
        next: &Track,
    ) -> bool {
        // Direct output: one stream on the card.
        if crossfade_seconds <= 0 || repeat == RepeatMode::One || is_direct {
            return false;
        }
        // An album heard in order stays gapless.
        if !shuffle && !current.album.is_empty() && current.album == next.album {
            return false;
        }
        // Too short to overlap.
        current.duration <= 0.0 || current.duration >= f64::from(crossfade_seconds) * 2.0 + 2.0
    }

    pub fn equal_power_gains(t: f64, base_vol: f64) -> (f64, f64) {
        let t = t.clamp(0.0, 1.0);
        let base = base_vol.clamp(0.0, 1.0);
        let theta = t * FRAC_PI_2;
        let fade_out = base * theta.cos().max(0.0).cbrt();
        let fade_in = base * theta.sin().max(0.0).cbrt();
        (fade_out, fade_in)
    }
}

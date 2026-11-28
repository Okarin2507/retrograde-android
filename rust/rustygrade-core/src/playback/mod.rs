use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Track;

mod crossfade;
mod editing;
mod navigation;
mod queue;
mod session;
mod tracker;
#[cfg(test)]
mod tests;

pub use crossfade::CrossfadeMath;
pub use tracker::PlayTracker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off = 0,
    All = 1,
    One = 2,
}

impl RepeatMode {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::All,
            2 => Self::One,
            _ => Self::Off,
        }
    }

    pub fn as_i32(self) -> i32 {
        self as i32
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

}

static SHUFFLE_NONCE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

/// A fresh non-zero xorshift seed: wall clock mixed with a per-call nonce, so two shuffles in the
/// same clock tick still differ.
fn shuffle_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x517c_c1b7_2722_0a95);
    let x = SHUFFLE_NONCE.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed) ^ nanos;
    if x == 0 { 0xdead_beef_cafe_babe } else { x }
}

/// Fisher–Yates shuffle. The PRNG is seeded once per call; the loop itself never reads the clock.
fn shuffle_slice<T>(slice: &mut [T]) {
    let mut x = shuffle_seed();
    for i in (1..slice.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let j = (x as usize) % (i + 1);
        slice.swap(i, j);
    }
}

/// The play queue, mirroring the desktop `PlaybackQueue`.
///
/// `tracks` is the play order: what follows the current track is what plays next, in shuffle too.
/// Shuffle scopes to the active queue (current album or playlist): turning it on keeps the current
/// track first and randomizes the rest; turning it off restores the queue from before, at the
/// current track.
///
/// Tracks queued by hand form a line right after the current track: ADD_TO_QUEUE joins the end of
/// that line, PLAY_NEXT goes to its front. The line plays before the rest of the queue.
/// What [`PlaybackQueue::on_player_advanced`] found when the player moved on into its queued file by itself.
#[derive(Debug, Clone)]
pub enum PlayerAdvance {
    /// The player entered the file it was handed: this track is now current and already playing.
    Followed(Track),
    /// The queue was edited right at the boundary and the player entered a file that is no longer next:
    /// the intended track is now current and has to be loaded (C# `OnAdvanced` → `PlayIndexAsync`).
    Reload(Track),
    /// Nothing is meant to follow: stop.
    Stop,
}

#[derive(Debug, Default)]
pub struct PlaybackQueue {
    /// Reference-counted: moving to another track only changes `index`; an edit copies the list
    /// once (`Arc::make_mut`).
    tracks: Arc<Vec<Track>>,
    /// The queue from before shuffle (C# `_unshuffledQueue`); `None` while not shuffling, or after
    /// a restart that did not keep it.
    unshuffled: Option<Vec<Track>>,
    index: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
    /// How many of the tracks right after the current one were queued by hand.
    hand: usize,
}

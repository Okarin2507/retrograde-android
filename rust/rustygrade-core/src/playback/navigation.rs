use std::sync::Arc;

use super::crossfade::CrossfadeMath;
use super::{PlayerAdvance, PlaybackQueue, RepeatMode, shuffle_slice};
use crate::Track;

impl PlaybackQueue {
    /// Returns the track index that should play next automatically (gapless / EOF), the current
    /// one on `RepeatMode::One`.
    pub fn auto_next_index(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        let cur = self.index?;
        if self.repeat == RepeatMode::One {
            return Some(cur);
        }
        if cur + 1 < self.tracks.len() {
            Some(cur + 1)
        } else if self.repeat == RepeatMode::All {
            Some(0)
        } else {
            None
        }
    }

    /// C# `CrossfadeApplies`: whether the change from the current track to `next` fades.
    pub fn crossfade_applies(&self, next: usize, crossfade_seconds: i32, is_direct: bool) -> bool {
        let Some(cur) = self.index else {
            return false;
        };
        if next == cur {
            return false;
        }
        let (Some(current), Some(following)) = (self.tracks.get(cur), self.tracks.get(next)) else {
            return false;
        };
        CrossfadeMath::should_crossfade(
            crossfade_seconds,
            self.repeat,
            is_direct,
            self.shuffle,
            current,
            following,
        )
    }

    pub fn step_next(&mut self, from_eof: bool) -> Option<Track> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let cur = match self.index {
            Some(i) => i,
            None => {
                self.hand = 0;
                self.index = Some(0);
                return self.tracks.first().cloned();
            }
        };
        if from_eof && self.repeat == RepeatMode::One {
            self.set_index(cur);
            return self.tracks.get(cur).cloned();
        }
        if cur + 1 < len {
            self.set_index(cur + 1);
            return self.tracks.get(cur + 1).cloned();
        }
        if self.repeat == RepeatMode::All {
            self.hand = 0;
            if self.shuffle && len > 1 {
                self.deal_new_round(cur);
            }
            self.index = Some(0);
            return self.tracks.first().cloned();
        }
        None
    }

    /// Every track has had its turn (`last` was the final one): deal a new shuffled round that does
    /// not open with the track just played.
    fn deal_new_round(&mut self, last: usize) {
        let Some(last_id) = self.tracks.get(last).map(|t| t.id.clone()) else {
            return;
        };
        let tracks = Arc::make_mut(&mut self.tracks);
        shuffle_slice(tracks.as_mut_slice());
        if tracks[0].id == last_id
            && let Some(other) = tracks.iter().position(|t| t.id != last_id)
        {
            tracks.swap(0, other);
        }
    }

    /// Whether moving from the current track to slot `to` wraps from the end of a repeating shuffled
    /// queue back to its start.
    fn wraps_shuffled_round(&self, to: usize) -> bool {
        self.repeat == RepeatMode::All
            && self.shuffle
            && to == 0
            && self.tracks.len() > 1
            && self.index.is_some_and(|c| c + 1 == self.tracks.len())
    }

    pub fn step_prev(&mut self) -> Option<Track> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let cur = self.index.unwrap_or(0);
        let target = if cur > 0 {
            cur - 1
        } else if self.repeat == RepeatMode::All {
            len - 1
        } else {
            0
        };
        self.set_index(target);
        self.tracks.get(target).cloned()
    }

    /// The player ran from the current file into the one queued behind it (`path`); `expected` is the
    /// slot that was handed to it as next (C# `_queuedNextIndex`). When the queue changed at the
    /// boundary (a track queued by hand just as the player moved on), its file is not the intended one:
    /// the intended next track becomes current instead and must be loaded, so the hand-queued line
    /// is not skipped.
    pub fn on_player_advanced(&mut self, path: &str, expected: Option<usize>) -> PlayerAdvance {
        if let Some(e) = expected
            && self.tracks.get(e).is_some_and(|t| t.file_path == path)
        {
            if self.wraps_shuffled_round(e) {
                // The player already plays slot 0: keep it first and deal the rest of the new round.
                self.hand = 0;
                shuffle_slice(&mut Arc::make_mut(&mut self.tracks)[1..]);
            }
            self.set_index(e);
            return PlayerAdvance::Followed(self.tracks[e].clone());
        }
        match self.auto_next_index() {
            Some(want) => {
                if self.wraps_shuffled_round(want)
                    && let Some(cur) = self.index
                {
                    self.hand = 0;
                    self.deal_new_round(cur);
                }
                self.set_index(want);
                PlayerAdvance::Reload(self.tracks[want].clone())
            }
            None => PlayerAdvance::Stop,
        }
    }
}

use std::sync::Arc;

use super::{PlaybackQueue, RepeatMode, shuffle_slice};
use crate::Track;

impl PlaybackQueue {
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn index(&self) -> Option<usize> {
        self.index
    }

    pub fn current(&self) -> Option<&Track> {
        self.index.and_then(|i| self.tracks.get(i))
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// How many upcoming tracks were queued by hand (they sit right after the current one).
    pub fn hand_len(&self) -> usize {
        self.hand
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat = self.repeat.cycle();
        self.repeat
    }

    pub fn can_go_next(&self) -> bool {
        match self.index {
            Some(i) => i + 1 < self.tracks.len() || (self.repeat == RepeatMode::All && !self.tracks.is_empty()),
            None => !self.tracks.is_empty(),
        }
    }

    pub fn can_go_previous(&self) -> bool {
        match self.index {
            Some(i) => i > 0 || (self.repeat == RepeatMode::All && self.tracks.len() > 1),
            None => false,
        }
    }

    /// A new current track (C# `NoteTrackChange`): moving on to the next track in order uses up one
    /// hand-queued track; a jump anywhere else ends the line (what it skipped stays as ordinary tracks).
    fn note_track_change(&mut self, new_index: usize) {
        let in_order = self.index.is_some_and(|i| i + 1 == new_index);
        self.hand = if in_order { self.hand.saturating_sub(1) } else { 0 };
    }

    pub(super) fn set_index(&mut self, new_index: usize) {
        self.note_track_change(new_index);
        self.index = Some(new_index);
    }

    #[cfg(test)]
    pub(super) fn set_index_for_test(&mut self, i: usize) {
        self.index = Some(i);
    }

    /// Sets the active queue from an album or playlist. When shuffle is enabled:
    /// - `Some(idx)` keeps that track first and shuffles the remaining tracks in `tracks`.
    /// - `None` shuffles all tracks in `tracks` and starts from a random track at index `0`.
    pub fn set_queue(&mut self, tracks: Vec<Track>, start_index: Option<usize>) {
        if tracks.is_empty() {
            self.clear();
            return;
        }
        self.hand = 0;
        if self.shuffle {
            self.unshuffled = Some(tracks.clone());
            self.tracks = Arc::new(tracks);
            self.index = start_index.map(|i| i.min(self.tracks.len() - 1));
            self.build_order();
            self.index = Some(0);
        } else {
            let idx = start_index.unwrap_or(0).min(tracks.len() - 1);
            self.tracks = Arc::new(tracks);
            self.index = Some(idx);
            self.unshuffled = None;
        }
    }

    pub fn jump_to(&mut self, idx: usize) -> Option<Track> {
        if idx < self.tracks.len() {
            self.set_index(idx);
            self.tracks.get(idx).cloned()
        } else {
            None
        }
    }

    pub fn toggle_shuffle(&mut self) -> bool {
        self.set_shuffle(!self.shuffle);
        self.shuffle
    }

    /// Toggles shuffle within the current queue (album or playlist).
    pub fn set_shuffle(&mut self, enabled: bool) {
        if self.shuffle == enabled {
            return;
        }
        self.shuffle = enabled;
        self.hand = 0; // the queue is rebuilt around the current track
        if enabled {
            if self.tracks.is_empty() {
                return;
            }
            self.unshuffled = Some(self.tracks.to_vec());
            self.build_order();
        } else {
            self.restore_unshuffled();
        }
    }

    /// Builds a fresh round within the current queue: the current track stays first when known,
    /// and every other track in the queue follows in random order.
    fn build_order(&mut self) {
        match self.index {
            Some(cur) if cur < self.tracks.len() => {
                let tracks = Arc::make_mut(&mut self.tracks);
                // O(n): move the current track to the front, then shuffle the rest in place.
                tracks.swap(0, cur);
                shuffle_slice(&mut tracks[1..]);
                self.index = Some(0);
            }
            _ => shuffle_slice(Arc::make_mut(&mut self.tracks).as_mut_slice()),
        }
    }

    /// Restores the queue from before shuffle, at the track now playing.
    fn restore_unshuffled(&mut self) {
        let Some(back) = self.unshuffled.take() else {
            return;
        };
        let Some(current) = self.current().cloned() else {
            self.tracks = Arc::new(back);
            self.index = None;
            return;
        };
        if let Some(idx) = back.iter().position(|t| t.id == current.id) {
            self.tracks = Arc::new(back);
            self.index = Some(idx);
        } else {
            let mut list = Vec::with_capacity(back.len() + 1);
            list.push(current);
            list.extend(back);
            self.tracks = Arc::new(list);
            self.index = Some(0);
        }
    }
}

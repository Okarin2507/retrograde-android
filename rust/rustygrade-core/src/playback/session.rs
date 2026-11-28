use std::sync::Arc;

use super::{PlaybackQueue, RepeatMode};
use crate::{SessionState, Track, TrackLookup};

impl PlaybackQueue {
    /// Restores a saved queue. A shuffled one is the library-wide queue already, so it is not spread
    /// again; the queue from before comes from `original_queue_ids` when it was saved.
    pub fn restore_session(
        &mut self,
        lookup: &TrackLookup<'_>,
        state: &SessionState,
        queue_ids: &[String],
        original_queue_ids: Option<&[String]>,
    ) -> Option<Track> {
        let resolve = |ids: &[String]| -> Vec<Track> {
            ids.iter()
                .filter_map(|id| lookup.get(id).cloned())
                .collect()
        };
        self.shuffle = state.shuffle;
        self.repeat = RepeatMode::from_i32(state.repeat_mode);
        self.hand = 0;
        self.unshuffled = if state.shuffle {
            original_queue_ids
                .map(resolve)
                .filter(|orig| !orig.is_empty())
        } else {
            None
        };
        self.tracks = Arc::new(resolve(queue_ids));

        if self.tracks.is_empty() {
            self.index = None;
            return None;
        }

        let matched = state
            .current_id
            .as_deref()
            .and_then(|cid| self.tracks.iter().position(|t| t.id == cid))
            .or_else(|| {
                (state.index >= 0 && (state.index as usize) < self.tracks.len())
                    .then_some(state.index as usize)
            })
            .or(Some(0));
        self.index = matched;
        self.current().cloned()
    }

    /// The playback state alone (current track, position, shuffle, repeat), without the queue.
    pub fn session_state(&self, position_s: f64) -> SessionState {
        SessionState {
            current_id: self.current().map(|t| t.id.clone()),
            index: self.index.map(|i| i as i32).unwrap_or(-1),
            position: position_s.max(0.0),
            shuffle: self.shuffle,
            repeat_mode: self.repeat.as_i32(),
        }
    }

    /// `(state, play order ids, ids of the queue from before shuffle)`; the last is empty when
    /// there is none.
    pub fn to_session(&self, position_s: f64) -> (SessionState, Vec<String>, Vec<String>) {
        let state = self.session_state(position_s);
        let active_ids = self.tracks.iter().map(|t| t.id.clone()).collect();
        let original_ids = self
            .unshuffled
            .iter()
            .flatten()
            .map(|t| t.id.clone())
            .collect();
        (state, active_ids, original_ids)
    }
}

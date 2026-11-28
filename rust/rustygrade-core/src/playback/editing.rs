use std::collections::HashSet;
use std::sync::Arc;

use super::PlaybackQueue;
use crate::{Track, TrackLookup};

impl PlaybackQueue {
    /// PLAY_NEXT: right after the current track, ahead of the tracks queued before. Returns `true`
    /// when nothing was queued and this started a new queue (the caller starts playback).
    pub fn play_next(&mut self, tracks: Vec<Track>) -> bool {
        self.enqueue_by_hand(tracks, true)
    }

    /// ADD_TO_QUEUE: after the current track and the tracks queued by hand before it, without
    /// interrupting playback. Returns `true` when this started a new queue.
    pub fn enqueue(&mut self, tracks: Vec<Track>) -> bool {
        self.enqueue_by_hand(tracks, false)
    }

    fn enqueue_by_hand(&mut self, list: Vec<Track>, at_front: bool) -> bool {
        if list.is_empty() {
            return false;
        }
        let Some(cur) = self.index.filter(|&i| i < self.tracks.len()) else {
            // Nothing queued: start playing.
            self.set_queue(list, Some(0));
            return true;
        };
        let upcoming = self.tracks.len() - cur - 1;
        let offset = if at_front { 0 } else { self.hand.min(upcoming) };
        self.keep_unshuffled_insert(&list, offset);
        let n = list.len();
        let at = cur + 1 + offset;
        Arc::make_mut(&mut self.tracks).splice(at..at, list);
        self.hand = self.hand.min(upcoming) + n;
        false
    }

    /// Tracks queued by hand go to the same place in the queue from before: after the current track
    /// (and the line).
    fn keep_unshuffled_insert(&mut self, list: &[Track], offset: usize) {
        let Some(current_id) = self.current().map(|t| t.id.clone()) else {
            return;
        };
        let Some(back) = self.unshuffled.as_mut() else {
            return;
        };
        let pos = match back.iter().position(|t| t.id == current_id) {
            Some(at) => (at + 1 + offset).min(back.len()),
            None => back.len(),
        };
        back.splice(pos..pos, list.iter().cloned());
    }

    /// Moves the track at play-order position `from` so that it ends up at position `to` (drag and
    /// drop in the queue sheet). Returns `false` when nothing moved.
    ///
    /// - The playing track stays the playing track: `index` follows it, also when it is the one
    ///   being moved (then the line of hand-queued tracks after it ends).
    /// - The hand-queued line is the run of tracks right after the current one. Dropping a track
    ///   inside that run adds it to the line; taking one out of the run shortens it.
    /// - The queue from before shuffle is left alone: a move reorders what plays now only.
    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        let len = self.tracks.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let cur = self.index.filter(|&i| i < len);

        // Where the line, the current index and the dropped track end up, in the list without `from`.
        let (mut new_index, mut new_hand) = (cur, self.hand);
        if let Some(c) = cur {
            if from == c {
                new_index = Some(to);
                new_hand = 0;
            } else {
                let in_line = from > c && from <= c + self.hand;
                let c_removed = if from < c { c - 1 } else { c };
                let hand_removed = self.hand - usize::from(in_line);
                // `to` is a position in the final list; relative to the list without `from` it is the
                // insertion index.
                if to <= c_removed {
                    new_index = Some(c_removed + 1);
                    new_hand = hand_removed;
                } else {
                    new_index = Some(c_removed);
                    // Dropped inside the line, or (when it came out of the line) on its last slot,
                    // it is in the line; dropped right behind a line it was never part of, it is not.
                    new_hand = hand_removed + usize::from(to <= c_removed + hand_removed + usize::from(in_line));
                }
            }
        }

        let tracks = Arc::make_mut(&mut self.tracks);
        let item = tracks.remove(from);
        tracks.insert(to, item);
        self.index = new_index;
        self.hand = new_hand;
        true
    }

    pub fn remove_at(&mut self, idx: usize) -> bool {
        if idx >= self.tracks.len() {
            return false;
        }
        if let Some(cur) = self.index {
            if idx == cur {
                // The track after it becomes current: one hand-queued track used up.
                self.hand = self.hand.saturating_sub(1);
            } else if idx > cur && idx - cur - 1 < self.hand {
                self.hand -= 1;
            }
        }
        let removed = Arc::make_mut(&mut self.tracks).remove(idx);
        if let Some(back) = self.unshuffled.as_mut() {
            // The queue from before keeps no more copies of this track than the play order still
            // has, so a duplicate id elsewhere is never dropped by mistake.
            let left = self.tracks.iter().filter(|t| t.id == removed.id).count();
            while back.iter().filter(|t| t.id == removed.id).count() > left {
                match back.iter().rposition(|t| t.id == removed.id) {
                    Some(at) => {
                        back.remove(at);
                    }
                    None => break,
                }
            }
        }
        self.index = match self.index {
            None => None,
            Some(_) if self.tracks.is_empty() => None,
            Some(cur) if idx < cur => Some(cur - 1),
            Some(cur) if idx == cur => Some(cur.min(self.tracks.len() - 1)),
            Some(cur) => Some(cur),
        };
        true
    }

    pub fn clear(&mut self) {
        self.unshuffled = None;
        self.tracks = Arc::default();
        self.index = None;
        self.hand = 0;
    }

    /// Track IDs referenced by either the active play order or the saved unshuffled queue.
    pub fn all_track_ids(&self) -> impl Iterator<Item = &str> {
        self.tracks
            .iter()
            .map(|t| t.id.as_str())
            .chain(self.unshuffled.iter().flat_map(|v| v.iter()).map(|t| t.id.as_str()))
    }

    /// Drops all tracks whose id is in `ids`, keeping `index` and `hand` consistent when the current
    /// track survives, or clearing playback state (`current_was_removed = true`) like C# `RemoveTracksAsync`.
    /// Returns `(queue_changed, current_was_removed)`.
    pub fn remove_tracks_by_id(&mut self, ids: &HashSet<String>) -> (bool, bool) {
        if ids.is_empty() {
            return (false, false);
        }
        let had_in_tracks = self.tracks.iter().any(|t| ids.contains(&t.id));
        let had_in_back = self
            .unshuffled
            .as_ref()
            .is_some_and(|b| b.iter().any(|t| ids.contains(&t.id)));
        if !had_in_tracks && !had_in_back {
            return (false, false);
        }

        let cur_idx = self.index.filter(|&i| i < self.tracks.len());
        let current_removed = cur_idx
            .and_then(|i| self.tracks.get(i))
            .is_some_and(|t| ids.contains(&t.id));

        if let Some(back) = self.unshuffled.as_mut() {
            back.retain(|t| !ids.contains(&t.id));
        }

        if current_removed {
            Arc::make_mut(&mut self.tracks).retain(|t| !ids.contains(&t.id));
            self.index = None;
            self.hand = 0;
            return (true, true);
        }

        let mut removed_before_cur = 0usize;
        let mut removed_in_hand = 0usize;
        if let Some(ci) = cur_idx {
            for (i, t) in self.tracks.iter().enumerate() {
                if ids.contains(&t.id) {
                    if i < ci {
                        removed_before_cur += 1;
                    } else if i > ci && (i - ci - 1) < self.hand {
                        removed_in_hand += 1;
                    }
                }
            }
        }
        Arc::make_mut(&mut self.tracks).retain(|t| !ids.contains(&t.id));
        if self.tracks.is_empty() {
            self.index = None;
            self.hand = 0;
        } else if let Some(ci) = cur_idx {
            let nc = (ci - removed_before_cur).min(self.tracks.len() - 1);
            let upcoming = self.tracks.len() - nc - 1;
            self.index = Some(nc);
            self.hand = self.hand.saturating_sub(removed_in_hand).min(upcoming);
        }
        (true, false)
    }

    pub fn refresh_from_library(&mut self, lookup: &TrackLookup<'_>) {
        let fresh = |t: &mut Track| {
            if let Some(f) = lookup.get(&t.id) {
                *t = f.clone();
            }
        };
        Arc::make_mut(&mut self.tracks).iter_mut().for_each(fresh);
        if let Some(back) = self.unshuffled.as_mut() {
            back.iter_mut().for_each(fresh);
        }
    }
}

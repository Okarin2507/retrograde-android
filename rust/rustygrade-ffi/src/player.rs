//! `PlayerEngine`: the play queue, play counting and session persistence behind the Media3 service.
//!
//! Rust owns the queue (order, shuffle across the library, the hand-queued line, repeat). The
//! Kotlin player only ever holds a small window: `[previous, current, next]`. After every call the
//! service reconciles that window with the returned [`PlayerState`].

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rustygrade_core::{
    PlayerAdvance, PlayStats, PlayTracker, PlaybackQueue, RepeatMode, SessionStore, Track,
};

use crate::dto::TrackInfo;
use crate::engine::{LibraryData, RustygradeEngine};
use crate::error::EngineError;

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlayerState {
    pub current: Option<TrackInfo>,
    /// What "previous" goes back to (`None`: restart the current track).
    pub previous: Option<TrackInfo>,
    /// What follows automatically. `None` when nothing does, or when the current track loops.
    pub next: Option<TrackInfo>,
    /// The current track is followed by itself (repeat one, or repeat all with one track): the
    /// player should loop the item instead of being handed a next one.
    pub loop_current: bool,
    /// The change from the current track to `next` should crossfade (the setting is on, the track
    /// is long enough, and it is not an album played in order, repeat one or direct USB output).
    pub crossfade_applies: bool,
    pub index: i32,
    pub queue_len: u32,
    /// Tracks queued by hand right after the current one.
    pub hand_len: u32,
    pub shuffle: bool,
    /// 0 off, 1 all, 2 one.
    pub repeat: i32,
    pub can_next: bool,
    pub can_previous: bool,
    /// Grows with every change to the queue or the current track.
    pub revision: u64,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum AdvanceKind {
    /// The player moved into the item it was handed: it is already playing.
    Followed,
    /// The queue changed right at the boundary: the intended track is current and must be loaded.
    Reload,
    /// Nothing is meant to follow.
    Stop,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AdvanceResult {
    pub kind: AdvanceKind,
    pub state: PlayerState,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct QueueEditResult {
    pub state: PlayerState,
    /// The queue was empty: this started a new queue and playback should begin.
    pub started_new_queue: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RestoredSession {
    pub state: PlayerState,
    pub position_s: f64,
}

struct Inner {
    queue: PlaybackQueue,
    tracker: PlayTracker,
    /// The slot last handed to the player as "next" (C# `_queuedNextIndex`).
    handed_next: Option<usize>,
    revision: u64,
    seen_generation: u64,
    /// Bumped by every queue edit; the queue is dirty while it differs from `saved_queue_seq`. A
    /// counter (not a flag) so an edit that lands while a save is writing is never marked clean.
    queue_seq: u64,
    /// The `queue_seq` whose queue was last written to `native-queue.json`.
    saved_queue_seq: u64,
    /// The track the play tracker is timing.
    tracker_track: Option<String>,
    crossfade_seconds: i32,
    /// Bit-perfect USB output: one stream only, never a crossfade.
    direct_output: bool,
}

#[derive(uniffi::Object)]
pub struct PlayerEngine {
    library: Arc<RustygradeEngine>,
    inner: Mutex<Inner>,
    /// Orders session saves (snapshot -> write -> mark saved) without holding `inner` over disk I/O.
    save_lock: Mutex<()>,
}

#[uniffi::export]
impl PlayerEngine {
    /// `library` must be the app's single `RustygradeEngine`: they share the library cache.
    #[uniffi::constructor]
    pub fn new(library: Arc<RustygradeEngine>) -> Arc<Self> {
        Arc::new(Self {
            library,
            inner: Mutex::new(Inner {
                queue: PlaybackQueue::default(),
                tracker: PlayTracker::default(),
                handed_next: None,
                revision: 0,
                seen_generation: u64::MAX,
                queue_seq: 0,
                saved_queue_seq: 0,
                tracker_track: None,
                crossfade_seconds: 0,
                direct_output: false,
            }),
            save_lock: Mutex::new(()),
        })
    }

    pub fn state(&self) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| Ok(inner.snapshot()))
    }

    // ---- starting playback ------------------------------------------------------------------

    /// Queue = the album, playing from `start_index` (or random start when `None` in shuffle).
    pub fn play_album(&self, key: String, start_index: Option<u32>) -> Result<PlayerState, EngineError> {
        self.with(|data, inner, _| {
            if let Some(album) = data.album(&key) {
                inner.queue.set_queue(album.tracks.clone(), start_index.map(|i| i as usize));
                inner.changed(true, true);
            }
            Ok(inner.snapshot())
        })
    }

    /// Shuffles the album and starts from a random track within it.
    pub fn play_album_shuffled(&self, key: String) -> Result<PlayerState, EngineError> {
        self.with(|data, inner, _| {
            if let Some(album) = data.album(&key) {
                inner.queue.set_shuffle(true);
                inner.queue.set_queue(album.tracks.clone(), None);
                inner.changed(true, true);
            }
            Ok(inner.snapshot())
        })
    }

    /// Queue = these tracks (favorites, a playlist, search results), starting at `start_index`
    /// (or random start when `None` in shuffle).
    pub fn play_tracks(&self, ids: Vec<String>, start_index: Option<u32>) -> Result<PlayerState, EngineError> {
        self.with(|data, inner, _| {
            let tracks = resolve(data, &ids);
            if !tracks.is_empty() {
                inner.queue.set_queue(tracks, start_index.map(|i| i as usize));
                inner.changed(true, true);
            }
            Ok(inner.snapshot())
        })
    }

    /// Shuffles these tracks and starts from a random track among them.
    pub fn play_tracks_shuffled(&self, ids: Vec<String>) -> Result<PlayerState, EngineError> {
        self.with(|data, inner, _| {
            let tracks = resolve(data, &ids);
            if !tracks.is_empty() {
                inner.queue.set_shuffle(true);
                inner.queue.set_queue(tracks, None);
                inner.changed(true, true);
            }
            Ok(inner.snapshot())
        })
    }

    // ---- queue edits ------------------------------------------------------------------------

    pub fn play_next(&self, ids: Vec<String>) -> Result<QueueEditResult, EngineError> {
        self.edit(ids, true)
    }

    pub fn enqueue(&self, ids: Vec<String>) -> Result<QueueEditResult, EngineError> {
        self.edit(ids, false)
    }

    pub fn remove_at(&self, index: u32) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            if inner.queue.remove_at(index as usize) {
                inner.changed(true, false);
            }
            Ok(inner.snapshot())
        })
    }

    pub fn clear_queue(&self) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            inner.queue.clear();
            inner.changed(true, false);
            Ok(inner.snapshot())
        })
    }

    /// One page of the play order, for the queue sheet (never the whole library at once).
    pub fn queue_page(&self, offset: u32, limit: u32) -> Result<Vec<TrackInfo>, EngineError> {
        self.with(|_, inner, _| {
            Ok(inner
                .queue
                .tracks()
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .map(TrackInfo::from)
                .collect())
        })
    }

    // ---- moving through the queue ---------------------------------------------------------

    /// Skip forward (button/lockscreen) or, with `from_eof`, step past a finished track.
    pub fn step_next(&self, from_eof: bool) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            if inner.queue.step_next(from_eof).is_some() {
                inner.changed(false, true);
            }
            Ok(inner.snapshot())
        })
    }

    /// Always moves back one track; the caller restarts the current track instead when it has
    /// played for more than 3 s (the desktop rule).
    pub fn step_prev(&self) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            if inner.queue.step_prev().is_some() {
                inner.changed(false, true);
            }
            Ok(inner.snapshot())
        })
    }

    pub fn jump_to(&self, index: u32) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            if inner.queue.jump_to(index as usize).is_some() {
                inner.changed(false, true);
            }
            Ok(inner.snapshot())
        })
    }

    /// The player entered the item it was handed (`file_path` of the track). Decides whether it is
    /// still the intended next track.
    pub fn on_player_advanced(&self, file_path: String) -> Result<AdvanceResult, EngineError> {
        self.with(|_, inner, _| {
            let kind = match inner.queue.on_player_advanced(&file_path, inner.handed_next) {
                PlayerAdvance::Followed(_) => AdvanceKind::Followed,
                PlayerAdvance::Reload(_) => AdvanceKind::Reload,
                PlayerAdvance::Stop => AdvanceKind::Stop,
            };
            // `Stop` changes nothing (the queue did not move): no new revision, no timer restart.
            if !matches!(kind, AdvanceKind::Stop) {
                inner.changed(false, true);
            }
            Ok(AdvanceResult { kind, state: inner.snapshot() })
        })
    }

    /// Drag and drop in the queue: the track at `from` goes to position `to` (play order). The
    /// playing track keeps playing; see `PlaybackQueue::move_item` for the line/index rules.
    pub fn move_item(&self, from: u32, to: u32) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            if inner.queue.move_item(from as usize, to as usize) {
                inner.changed(true, false);
            }
            Ok(inner.snapshot())
        })
    }

    /// The crossfade length (0 = off) and whether output is direct (bit-perfect USB), which
    /// disables it. Takes effect on the next state.
    pub fn set_crossfade(&self, seconds: i32, direct_output: bool) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            inner.crossfade_seconds = seconds.clamp(0, 12);
            inner.direct_output = direct_output;
            inner.changed(false, false);
            Ok(inner.snapshot())
        })
    }

    // ---- shuffle / repeat -------------------------------------------------------------------

    pub fn toggle_shuffle(&self) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            inner.queue.toggle_shuffle();
            inner.changed(true, false);
            Ok(inner.snapshot())
        })
    }

    pub fn set_shuffle(&self, enabled: bool) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            if inner.queue.shuffle() != enabled {
                inner.queue.set_shuffle(enabled);
                inner.changed(true, false);
            }
            Ok(inner.snapshot())
        })
    }

    /// 0 off, 1 all, 2 one.
    pub fn set_repeat(&self, repeat: i32) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            inner.queue.set_repeat(RepeatMode::from_i32(repeat));
            inner.changed(false, false);
            Ok(inner.snapshot())
        })
    }

    pub fn cycle_repeat(&self) -> Result<PlayerState, EngineError> {
        self.with(|_, inner, _| {
            inner.queue.cycle_repeat();
            inner.changed(false, false);
            Ok(inner.snapshot())
        })
    }

    // ---- play counting ------------------------------------------------------------------

    /// A track began playing (also a repeat-one loop): restart the listening timer.
    pub fn note_track_started(&self) -> Result<(), EngineError> {
        self.with_queue(|inner| {
            let id = inner.queue.current().map(|t| t.id.clone());
            inner.tracker.reset(id.clone());
            inner.tracker_track = id;
            Ok(())
        })
    }

    /// Feed the playback position (poll at most every second, only while playing). Returns `true`
    /// when the track was counted as played (50% or 4 min) and `native-stats.json` was updated.
    pub fn on_position(&self, position_s: f64, duration_s: f64, is_playing: bool) -> Result<bool, EngineError> {
        // Decide under the lock, record after releasing it: `PlayStats::update` reads, rewrites and
        // backs up native-stats.json (and waits for any import holding its lock), and every other
        // player call would queue up behind that.
        let counted = self.with_queue(|inner| Ok(inner.tracker.on_time_pos(position_s, duration_s, is_playing)))?;
        let Some(id) = counted else {
            return Ok(false);
        };
        PlayStats::update(|stats| stats.record_now(&id));
        Ok(true)
    }

    // ---- session persistence ----------------------------------------------------------------

    /// Persist queue (when it changed), current track, position, shuffle and repeat.
    pub fn save_session(&self, position_s: f64) -> Result<(), EngineError> {
        // One save at a time, so an older snapshot can never be written after a newer one.
        let _order = self.save_lock.lock().unwrap_or_else(|p| p.into_inner());
        let (state, queue, seq) = self.with_queue(|inner| {
            let dirty = inner.queue_seq != inner.saved_queue_seq;
            // Collecting the id lists walks (and clones) the whole queue: only do it when it changed.
            if dirty {
                let (state, ids, original) = inner.queue.to_session(position_s);
                Ok((state, Some((ids, original)), inner.queue_seq))
            } else {
                Ok((inner.queue.session_state(position_s), None, inner.queue_seq))
            }
        })?;
        // The session files are written with `inner` released.
        if let Some((ids, original)) = &queue {
            SessionStore::save_queue(ids);
            SessionStore::save_original_queue(original);
        }
        SessionStore::save_state(&state);
        if queue.is_some() {
            // Clean only up to what was written: an edit made meanwhile bumped `queue_seq` past `seq`.
            self.with_queue(|inner| {
                inner.saved_queue_seq = inner.saved_queue_seq.max(seq);
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Rebuilds the saved queue (tracks deleted since are skipped). `None` when there is nothing to
    /// resume. Does not start playback: the caller prepares the player paused at `position_s`.
    pub fn restore_session(&self) -> Result<Option<RestoredSession>, EngineError> {
        self.with(|data, inner, _| {
            let Some((state, ids)) = SessionStore::load() else {
                return Ok(None);
            };
            let original = SessionStore::load_original_queue();
            let current = inner.queue.restore_session(&data.lookup(), &state, &ids, original.as_deref());
            if current.is_none() {
                return Ok(None);
            }
            inner.changed(false, true);
            Ok(Some(RestoredSession { state: inner.snapshot(), position_s: state.position.max(0.0) }))
        })
    }
}

impl PlayerEngine {
    /// For calls that never read the library (position polls, session saves): they run on the
    /// player's main thread every second, so they must not wait for `library.json` to be re-parsed
    /// after a scan.
    fn with_queue<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R, EngineError>) -> Result<R, EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut inner)
    }

    fn edit(&self, ids: Vec<String>, at_front: bool) -> Result<QueueEditResult, EngineError> {
        self.with(|data, inner, _| {
            let tracks = resolve(data, &ids);
            if tracks.is_empty() {
                // Nothing to add (unknown ids): not a change, so no revision bump and no dirty queue.
                return Ok(QueueEditResult { state: inner.snapshot(), started_new_queue: false });
            }
            let started = if at_front {
                inner.queue.play_next(tracks)
            } else {
                inner.queue.enqueue(tracks)
            };
            inner.changed(true, false);
            Ok(QueueEditResult { state: inner.snapshot(), started_new_queue: started })
        })
    }

    /// Runs `f` with the (fresh) library and the locked queue state. A scan or delete since the last
    /// call is folded into the queue first: tracks are refreshed, vanished ones dropped.
    fn with<R>(
        &self,
        f: impl FnOnce(&LibraryData, &mut Inner, &Arc<RustygradeEngine>) -> Result<R, EngineError>,
    ) -> Result<R, EngineError> {
        let data = self.library.library()?;
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        // The generation the data was loaded under: a scan that finished while it loaded must still
        // be folded into the queue on the next call.
        let generation = data.generation;
        if inner.seen_generation != generation {
            inner.seen_generation = generation;
            let lookup = data.lookup();
            inner.queue.refresh_from_library(&lookup);
            let gone: HashSet<String> = inner
                .queue
                .all_track_ids()
                .filter(|id| !lookup.contains(id))
                .map(str::to_owned)
                .collect();
            if !gone.is_empty() {
                inner.queue.remove_tracks_by_id(&gone);
                // Slots shifted: the slot last handed to the player no longer means the same track.
                inner.handed_next = None;
                inner.changed(true, false);
            }
        }
        f(&data, &mut inner, &self.library)
    }
}

fn resolve(data: &LibraryData, ids: &[String]) -> Vec<Track> {
    let lookup = data.lookup();
    ids.iter().filter_map(|id| lookup.get(id).cloned()).collect()
}

impl Inner {
    /// Called after any queue or current-track change.
    ///
    /// `restart`: a track was (re)started, so the listening timer starts over even when the id is
    /// the same as before. Otherwise it only restarts if the current track really changed (an edit
    /// that merely adds to the queue must not wipe the time already heard).
    fn changed(&mut self, queue_edited: bool, restart: bool) {
        self.revision += 1;
        if queue_edited {
            self.queue_seq += 1;
        }
        let id = self.queue.current().map(|t| t.id.clone());
        if restart || id != self.tracker_track {
            self.tracker.reset(id.clone());
            self.tracker_track = id;
        }
    }

    fn snapshot(&mut self) -> PlayerState {
        let q = &self.queue;
        let tracks = q.tracks();
        let cur = q.index().filter(|&i| i < tracks.len());
        let auto = q.auto_next_index();
        let loop_current = cur.is_some() && auto == cur;
        let next_index = auto.filter(|&i| Some(i) != cur);
        self.handed_next = next_index;
        let previous = cur.and_then(|i| {
            if i > 0 {
                Some(i - 1)
            } else if q.repeat() == RepeatMode::All && tracks.len() > 1 {
                Some(tracks.len() - 1)
            } else {
                None
            }
        });
        PlayerState {
            current: cur.map(|i| TrackInfo::from(&tracks[i])),
            previous: previous.map(|i| TrackInfo::from(&tracks[i])),
            next: next_index.map(|i| TrackInfo::from(&tracks[i])),
            loop_current,
            crossfade_applies: next_index.is_some_and(|n| q.crossfade_applies(n, self.crossfade_seconds, self.direct_output)),
            index: cur.map(|i| i as i32).unwrap_or(-1),
            queue_len: tracks.len() as u32,
            hand_len: q.hand_len() as u32,
            shuffle: q.shuffle(),
            repeat: q.repeat().as_i32(),
            can_next: q.can_go_next(),
            can_previous: q.can_go_previous(),
            revision: self.revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use rustygrade_core::testing::{Fixture, TestEnv, write_flac};

    use super::*;
    use crate::dto::ScanRequest;

    /// Album A: a1 a2 a3, album B: b1 b2 (one second each).
    fn setup(env: &TestEnv) -> (Arc<RustygradeEngine>, Arc<PlayerEngine>) {
        for (album, artist, n) in [("A", "X", 3), ("B", "Y", 2)] {
            for i in 1..=n {
                let name = format!("{}{i}", album.to_lowercase());
                write_flac(&env.music.join(format!("{album}/{i:02}.flac")), &Fixture::new(&name, artist, album, i as u32));
            }
        }
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        let req = ScanRequest { dirs: vec![env.music.to_string_lossy().into_owned()], ..ScanRequest::default() };
        lib.scan_sources(req, None, None).unwrap();
        let player = PlayerEngine::new(lib.clone());
        (lib, player)
    }

    fn title(t: &Option<TrackInfo>) -> &str {
        t.as_ref().map(|t| t.title.as_str()).unwrap_or("-")
    }

    fn id_of(lib: &RustygradeEngine, title: &str) -> String {
        let hits = lib.search_library(title.to_owned()).unwrap();
        hits.tracks.into_iter().find(|t| t.title == title).expect("track").id
    }

    #[test]
    fn album_window_is_previous_current_next() {
        let env = TestEnv::new("player-window");
        let (_lib, p) = setup(&env);
        assert!(p.state().unwrap().current.is_none());

        let s = p.play_album("A".into(), Some(0)).unwrap();
        assert_eq!((title(&s.previous), title(&s.current), title(&s.next)), ("-", "a1", "a2"));
        assert_eq!((s.index, s.queue_len), (0, 3));
        assert!(s.can_next && !s.can_previous && !s.loop_current);

        let s = p.step_next(false).unwrap();
        assert_eq!((title(&s.previous), title(&s.current), title(&s.next)), ("a1", "a2", "a3"));
        let s = p.step_next(false).unwrap();
        assert_eq!((title(&s.current), title(&s.next)), ("a3", "-"), "repeat off ends the queue");
        assert!(!s.can_next);
        let s = p.step_prev().unwrap();
        assert_eq!(title(&s.current), "a2");
        let s = p.jump_to(0).unwrap();
        assert_eq!(title(&s.current), "a1");
        assert!(s.revision > 0);
    }

    #[test]
    fn repeat_modes_shape_the_next_slot() {
        let env = TestEnv::new("player-repeat");
        let (_lib, p) = setup(&env);
        p.play_album("A".into(), Some(2)).unwrap();

        let s = p.set_repeat(1).unwrap();
        assert_eq!(title(&s.next), "a1", "repeat all wraps");
        assert_eq!(title(&p.step_next(false).unwrap().current), "a1");
        p.jump_to(0).unwrap();
        assert_eq!(title(&p.step_prev().unwrap().current), "a3", "previous wraps too");

        p.jump_to(0).unwrap();
        let s = p.set_repeat(2).unwrap();
        assert!(s.loop_current && s.next.is_none(), "repeat one loops the item");
        assert_eq!(title(&p.step_next(false).unwrap().current), "a2", "skip still moves on");
        assert_eq!(title(&p.step_next(true).unwrap().current), "a2", "but a finished track repeats");

        // Repeat all on a one-track queue loops the item as well.
        let one = p.play_tracks(vec![id_of(&_lib, "b1")], Some(0)).unwrap();
        assert_eq!(one.queue_len, 1);
        p.set_repeat(1).unwrap();
        assert!(p.state().unwrap().loop_current);
        assert_eq!(p.cycle_repeat().unwrap().repeat, 2);
        assert_eq!(p.cycle_repeat().unwrap().repeat, 0);
    }

    #[test]
    fn hand_queue_changes_the_next_slot_and_reports_a_fresh_queue() {
        let env = TestEnv::new("player-hand");
        let (lib, p) = setup(&env);
        let started = p.enqueue(vec![id_of(&lib, "b1")]).unwrap();
        assert!(started.started_new_queue, "nothing was queued: this starts playback");
        assert_eq!(title(&started.state.current), "b1");

        p.play_album("A".into(), Some(0)).unwrap();
        let r = p.play_next(vec![id_of(&lib, "b2")]).unwrap();
        assert!(!r.started_new_queue);
        assert_eq!((title(&r.state.current), title(&r.state.next)), ("a1", "b2"));
        assert_eq!(r.state.hand_len, 1);
        let r = p.enqueue(vec![id_of(&lib, "b1")]).unwrap();
        assert_eq!(title(&r.state.next), "b2", "enqueue goes behind the hand line");
        assert_eq!(r.state.queue_len, 5);

        let s = p.step_next(false).unwrap();
        assert_eq!((title(&s.current), s.hand_len, title(&s.next)), ("b2", 1, "b1"));
        let page = p.queue_page(1, 2).unwrap();
        assert_eq!(page.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["b2", "b1"]);
    }

    #[test]
    fn shuffle_keeps_the_current_track_and_round_trips() {
        let env = TestEnv::new("player-shuffle");
        let (lib, p) = setup(&env);
        p.play_album("A".into(), Some(1)).unwrap();
        let s = p.toggle_shuffle().unwrap();
        assert!(s.shuffle);
        assert_eq!(title(&s.current), "a2", "shuffle keeps what is playing");
        assert_eq!(s.queue_len, 3, "shuffle stays scoped to album A");
        let s = p.set_shuffle(false).unwrap();
        assert!(!s.shuffle);
        assert_eq!((title(&s.current), s.queue_len, s.index), ("a2", 3, 1), "back to the album at the same track");

        let s = p.play_album_shuffled("B".into()).unwrap();
        assert!(s.shuffle);
        assert_eq!(s.queue_len, 2);
        assert!(matches!(title(&s.current), "b1" | "b2"));

        let b_ids = vec![id_of(&lib, "b1"), id_of(&lib, "b2")];
        let s = p.play_tracks_shuffled(b_ids).unwrap();
        assert!(s.shuffle);
        assert_eq!(s.queue_len, 2);
        assert!(matches!(title(&s.current), "b1" | "b2"));
    }

    #[test]
    fn gapless_advance_follows_or_reloads_when_the_queue_changed_at_the_boundary() {
        let env = TestEnv::new("player-advance");
        let (lib, p) = setup(&env);
        let s = p.play_album("A".into(), Some(0)).unwrap();
        let next_path = s.next.as_ref().unwrap().file_path.clone();

        let r = p.on_player_advanced(next_path).unwrap();
        assert!(matches!(r.kind, AdvanceKind::Followed));
        assert_eq!(title(&r.state.current), "a2");

        // A track is queued just as the player moves into the item it was handed before.
        let handed = r.state.next.as_ref().unwrap().file_path.clone(); // a3
        p.play_next(vec![id_of(&lib, "b1")]).unwrap();
        let r = p.on_player_advanced(handed).unwrap();
        assert!(matches!(r.kind, AdvanceKind::Reload), "{:?}", r.kind);
        assert_eq!(title(&r.state.current), "b1", "the hand-queued track is not skipped");

        p.play_album("A".into(), Some(2)).unwrap();
        let r = p.on_player_advanced("/anything".into()).unwrap();
        assert!(matches!(r.kind, AdvanceKind::Stop), "nothing follows the last track");
    }

    #[test]
    fn a_play_is_counted_once_and_queue_edits_do_not_reset_the_timer() {
        let env = TestEnv::new("player-tracker");
        let (lib, p) = setup(&env);
        let s = p.play_album("A".into(), Some(0)).unwrap();
        let id = s.current.unwrap().id;

        // One-second track: it has to be heard for ~0.5 s.
        assert!(!p.on_position(0.0, 1.0, true).unwrap());
        assert!(!p.on_position(0.3, 1.0, true).unwrap());
        p.play_next(vec![id_of(&lib, "b1")]).unwrap(); // must not wipe the 0.3 s already heard
        assert!(p.on_position(0.6, 1.0, true).unwrap(), "counted");
        assert!(!p.on_position(0.9, 1.0, true).unwrap(), "only once per play");

        let stats = lib.load_stats();
        let stat = stats.tracks.iter().find(|s| s.track_id == id).expect("recorded");
        assert_eq!(stat.count, 1);

        // Seeking or pausing never counts.
        p.step_next(false).unwrap();
        assert!(!p.on_position(0.0, 1.0, true).unwrap());
        assert!(!p.on_position(100.0, 1.0, true).unwrap(), "a jump is a seek");
        assert!(!p.on_position(100.4, 1.0, false).unwrap(), "paused time is not listening time");
    }

    #[test]
    fn session_round_trips_and_survives_deleted_tracks() {
        let env = TestEnv::new("player-session");
        let (lib, p) = setup(&env);
        assert!(p.restore_session().unwrap().is_none(), "nothing saved yet");

        p.play_album("B".into(), Some(0)).unwrap();
        p.enqueue(vec![id_of(&lib, "a3")]).unwrap();
        p.step_next(false).unwrap();
        p.set_repeat(1).unwrap();
        p.save_session(12.5).unwrap();

        let fresh = PlayerEngine::new(lib.clone());
        let r = fresh.restore_session().unwrap().expect("restored");
        // Queue after the edits: [b1, a3 (hand-queued), b2]; one step on is a3.
        assert_eq!(title(&r.state.current), "a3");
        assert_eq!((r.position_s, r.state.repeat, r.state.queue_len), (12.5, 1, 3));
        assert_eq!(title(&r.state.next), "b2");

        // Album B is deleted before the app comes back: the saved track is gone.
        lib.delete_albums(vec!["B".into()]).unwrap();
        let fresh = PlayerEngine::new(lib.clone());
        let r = fresh.restore_session().unwrap().expect("a queue is left");
        assert_eq!(r.state.queue_len, 1);
        assert_eq!(title(&r.state.current), "a3", "falls back to a surviving track");

        // And a running queue drops tracks that vanish from the library.
        p.play_album("A".into(), Some(0)).unwrap();
        p.enqueue(vec![id_of(&lib, "a1")]).unwrap();
        lib.delete_albums(vec!["A".into()]).unwrap();
        let s = p.state().unwrap();
        assert!(s.current.is_none() && s.queue_len == 0, "{s:?}");
    }

    #[test]
    fn removing_the_current_track_moves_on_and_clear_empties() {
        let env = TestEnv::new("player-remove");
        let (_lib, p) = setup(&env);
        p.play_album("A".into(), Some(0)).unwrap();
        let s = p.remove_at(0).unwrap();
        assert_eq!((title(&s.current), s.queue_len), ("a2", 2));
        let s = p.clear_queue().unwrap();
        assert!(s.current.is_none() && !s.can_next);
    }

    #[test]
    fn crossfade_applies_only_where_the_desktop_rule_allows_it() {
        let env = TestEnv::new("player-crossfade");
        for (album, n) in [("A", 3u32), ("B", 2)] {
            for i in 1..=n {
                let name = format!("{}{i}", album.to_lowercase());
                write_flac(
                    &env.music.join(format!("{album}/{i:02}.flac")),
                    &Fixture::new(&name, "X", album, i).with_seconds(60),
                );
            }
        }
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        let req = ScanRequest { dirs: vec![env.music.to_string_lossy().into_owned()], ..ScanRequest::default() };
        lib.scan_sources(req, None, None).unwrap();
        let p = PlayerEngine::new(lib.clone());

        let s = p.play_album("A".into(), Some(0)).unwrap();
        assert!(!s.crossfade_applies, "off by default");
        assert!(!p.set_crossfade(4, false).unwrap().crossfade_applies, "an album heard in order stays gapless");

        // The next track comes from another album: now it fades.
        let b1 = lib.search_library("b1".into()).unwrap().tracks[0].id.clone();
        let s = p.play_next(vec![b1]).unwrap().state;
        assert_eq!(title(&s.next), "b1");
        assert!(s.crossfade_applies);
        assert!(!p.set_crossfade(4, true).unwrap().crossfade_applies, "never on direct USB output");
        assert!(p.set_crossfade(4, false).unwrap().crossfade_applies);
        assert!(!p.set_crossfade(0, false).unwrap().crossfade_applies);
        p.set_crossfade(4, false).unwrap();
        assert!(!p.set_repeat(2).unwrap().crossfade_applies, "repeat one loops, no fade");
        p.set_repeat(0).unwrap();

        // A shuffled queue is not "an album in order": it fades between tracks of the same album too.
        let s = p.toggle_shuffle().unwrap();
        assert!(s.shuffle && s.crossfade_applies);
        // Tracks shorter than twice the fade plus two seconds do not fade.
        assert!(p.set_crossfade(12, false).unwrap().crossfade_applies, "60 s >= 26 s");
    }

    #[test]
    fn move_item_reorders_the_queue_and_updates_the_window() {
        let env = TestEnv::new("player-move");
        let (_lib, p) = setup(&env);
        p.play_album("A".into(), Some(0)).unwrap(); // a1 a2 a3, a1 playing
        let s = p.move_item(2, 1).unwrap(); // a3 right after the current track
        assert_eq!((title(&s.current), title(&s.next)), ("a1", "a3"));
        let order: Vec<String> = p.queue_page(0, 10).unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(order, ["a1", "a3", "a2"]);
        let before = s.revision;
        assert_eq!(p.move_item(1, 1).unwrap().revision, before, "a no-op move changes nothing");
        let s = p.move_item(0, 2).unwrap(); // the playing track dragged to the end
        assert_eq!((title(&s.current), s.index, s.next.is_none()), ("a1", 2, true));
        assert_eq!(title(&s.previous), "a2");
    }

    #[test]
    fn enqueueing_nothing_is_not_a_change() {
        let env = TestEnv::new("player-edit-empty");
        let (_lib, p) = setup(&env);
        let s = p.play_album("A".into(), Some(0)).unwrap();
        p.save_session(0.0).unwrap();
        let _ = std::fs::remove_file(SessionStore::queue_path());
        let r = p.enqueue(vec!["/not/in/the/library.flac".into()]).unwrap();
        assert!(!r.started_new_queue);
        assert_eq!(r.state.revision, s.revision, "unknown ids change nothing");
        let r = p.play_next(Vec::new()).unwrap();
        assert_eq!(r.state.revision, s.revision);
        p.save_session(1.0).unwrap();
        assert!(!SessionStore::queue_path().exists(), "the queue was not marked dirty");
    }

    #[test]
    fn stop_does_not_bump_the_revision_or_restart_the_timer() {
        let env = TestEnv::new("player-stop");
        let (_lib, p) = setup(&env);
        p.play_album("A".into(), Some(2)).unwrap(); // the last track, repeat off
        assert!(!p.on_position(0.0, 1.0, true).unwrap());
        assert!(!p.on_position(0.3, 1.0, true).unwrap());
        let before = p.state().unwrap().revision;
        let r = p.on_player_advanced("/anything".into()).unwrap();
        assert!(matches!(r.kind, AdvanceKind::Stop));
        assert_eq!(r.state.revision, before);
        assert!(p.on_position(0.6, 1.0, true).unwrap(), "the 0.3 s already heard still counts");
    }

    #[test]
    fn queue_edits_dirty_the_saved_queue_exactly_once() {
        let env = TestEnv::new("player-dirty");
        let (lib, p) = setup(&env);
        p.play_album("A".into(), Some(0)).unwrap();
        p.save_session(0.0).unwrap();
        let queue = SessionStore::queue_path();
        assert!(queue.exists());
        // Nothing changed: the queue file is not rewritten (but the small state file is).
        std::fs::remove_file(&queue).unwrap();
        p.save_session(2.0).unwrap();
        assert!(!queue.exists());
        assert_eq!(SessionStore::load().map(|(s, _)| s.position), None, "no queue file: no session");
        // An edit makes it dirty again, and one save cleans it.
        p.enqueue(vec![id_of(&lib, "b1")]).unwrap();
        p.save_session(3.0).unwrap();
        let (state, ids) = SessionStore::load().expect("saved");
        assert_eq!((state.position, ids.len()), (3.0, 4));
        std::fs::remove_file(&queue).unwrap();
        p.save_session(4.0).unwrap();
        assert!(!queue.exists(), "clean after the save that wrote it");
    }

    #[test]
    fn concurrent_saves_and_edits_never_lose_a_queue_change() {
        let env = TestEnv::new("player-save-race");
        let (lib, p) = setup(&env);
        p.play_album("A".into(), Some(0)).unwrap();
        let extra: Vec<String> = ["a2", "a3", "b1", "b2"].iter().map(|t| id_of(&lib, t)).collect();
        std::thread::scope(|s| {
            let saver = s.spawn(|| {
                for i in 0..40 {
                    p.save_session(f64::from(i)).unwrap();
                }
            });
            for id in &extra {
                p.enqueue(vec![id.clone()]).unwrap();
            }
            saver.join().unwrap();
        });
        p.save_session(99.0).unwrap();
        let (_, ids) = SessionStore::load().expect("saved");
        assert_eq!(ids.len(), 3 + extra.len(), "the last save has every edit");
    }

    #[test]
    fn other_player_calls_are_not_blocked_by_play_counting_io() {
        let env = TestEnv::new("player-no-io-lock");
        let (_lib, p) = setup(&env);
        p.play_album("A".into(), Some(0)).unwrap();
        // Heard long enough that the next poll counts the play (and so needs the stats lock).
        assert!(!p.on_position(0.0, 1.0, true).unwrap());
        assert!(!p.on_position(0.3, 1.0, true).unwrap());
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            // Hold the stats read-modify-write lock, as an import or remap would.
            s.spawn(|| {
                PlayStats::with_lock(|_| {
                    held_tx.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(600));
                });
            });
            held_rx.recv().unwrap();
            // This call counts a play, so it waits for the stats lock...
            let counter = s.spawn(|| p.on_position(0.6, 1.0, true));
            std::thread::sleep(std::time::Duration::from_millis(100));
            // ...but must not keep the queue locked while it waits.
            let started = std::time::Instant::now();
            let state = p.state().unwrap();
            assert!(started.elapsed() < std::time::Duration::from_millis(400), "{:?}", started.elapsed());
            assert_eq!(title(&state.current), "a1");
            assert!(counter.join().unwrap().unwrap(), "the play was counted once the lock was free");
        });
    }

    #[test]
    fn deleting_queued_tracks_forgets_the_handed_slot() {
        let env = TestEnv::new("player-handed");
        let (lib, p) = setup(&env);
        let s = p.play_album("A".into(), Some(0)).unwrap(); // hands a2 over
        let handed = s.next.unwrap().file_path;
        // The library changes under the queue: album B is deleted and the queue is folded on the next call.
        p.enqueue(vec![id_of(&lib, "b1")]).unwrap();
        lib.delete_albums(vec!["B".into()]).unwrap();
        let state = p.state().unwrap();
        assert_eq!(state.queue_len, 3, "b1 vanished from the queue");
        // The player entering the previously handed file is still understood.
        let r = p.on_player_advanced(handed).unwrap();
        assert!(matches!(r.kind, AdvanceKind::Followed | AdvanceKind::Reload), "{:?}", r.kind);
        assert_eq!(title(&r.state.current), "a2");
    }

    #[test]
    fn reconciliation_prunes_vanished_tracks_from_unshuffled_queue_too() {
        let env = TestEnv::new("player-reconcile-unshuffled");
        let (lib, p) = setup(&env);
        let a_ids = vec![id_of(&lib, "a1"), id_of(&lib, "a2"), id_of(&lib, "a3")];
        let b1 = id_of(&lib, "b1");
        let mut mixed = a_ids;
        mixed.push(b1);
        p.play_tracks(mixed, Some(0)).unwrap();
        p.toggle_shuffle().unwrap();
        // Remove B from library, then drop b1 from active queue? Better: simulate library delete directly:
        lib.delete_albums(vec!["B".into()]).unwrap();
        let s = p.set_shuffle(false).unwrap();
        assert_eq!(s.queue_len, 3);
        let titles: Vec<String> = p.queue_page(0, 10).unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, vec!["a1", "a2", "a3"]);
    }
}

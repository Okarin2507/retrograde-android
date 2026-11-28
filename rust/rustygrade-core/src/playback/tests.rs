use super::*;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::{Album, build_track_index};
use crate::{SessionState, Track, TrackLookup};

fn mk_track(id: &str, album: &str, duration: f64) -> Track {
    Track {
        id: id.to_owned(),
        title: id.to_owned(),
        artist: "Artist".to_owned(),
        album: album.to_owned(),
        duration,
        file_path: id.to_owned(),
        play_uri: String::new(),
        replay_gain: Default::default(),
        track_number: 1,
        disc_number: 1,
        codec: "FLAC".to_owned(),
        sample_rate: 44100,
        bits_per_sample: 16,
        channels: 2,
        cover_thumb_path: Some(PathBuf::from("/tmp/a.jpg")),
        cover_full_path: None,
    }
}

fn mk_album(name: &str, ids: &[&str]) -> Album {
    Album {
        name: name.to_owned(),
        artist: "Artist".to_owned(),
        cover_thumb_path: None,
        cover_full_path: None,
        year: 2024,
        added_at: 1,
        tracks: ids.iter().map(|id| mk_track(id, name, 200.0)).collect(),
    }
}

/// Two albums: A (a1..a4) and B (b1..b3).
fn library() -> Vec<Album> {
    vec![
        mk_album("A", &["/a/1", "/a/2", "/a/3", "/a/4"]),
        mk_album("B", &["/b/1", "/b/2", "/b/3"]),
    ]
}

fn ids(q: &PlaybackQueue) -> Vec<&str> {
    q.tracks().iter().map(|t| t.id.as_str()).collect()
}

fn upcoming(q: &PlaybackQueue) -> Vec<&str> {
    let cur = q.index().unwrap();
    q.tracks()[cur + 1..].iter().map(|t| t.id.as_str()).collect()
}

fn queue_of(n: usize, start: usize) -> PlaybackQueue {
    let mut q = PlaybackQueue::default();
    let tracks = (1..=n).map(|i| mk_track(&format!("t{i}"), "A", 200.0)).collect();
    q.set_queue(tracks, Some(start));
    q
}

fn order(q: &PlaybackQueue) -> String {
    q.tracks().iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(" ")
}

#[test]
fn move_item_ignores_bad_or_noop_moves() {
    let mut q = queue_of(4, 1);
    assert!(!q.move_item(2, 2));
    assert!(!q.move_item(9, 0));
    assert!(!q.move_item(0, 9));
    assert_eq!(order(&q), "t1 t2 t3 t4");
    assert_eq!(q.index(), Some(1));
}

#[test]
fn move_item_keeps_the_playing_track_when_others_cross_it() {
    let mut q = queue_of(5, 2); // t3 playing
    assert!(q.move_item(0, 4)); // t1 jumps from before to after the current track
    assert_eq!(order(&q), "t2 t3 t4 t5 t1");
    assert_eq!(q.current().unwrap().id, "t3");
    assert!(q.move_item(4, 0)); // and back to the front
    assert_eq!(order(&q), "t1 t2 t3 t4 t5");
    assert_eq!(q.current().unwrap().id, "t3");
    assert!(q.move_item(3, 1)); // t4 from after to before the current track
    assert_eq!(order(&q), "t1 t4 t2 t3 t5");
    assert_eq!(q.current().unwrap().id, "t3");
}

#[test]
fn moving_the_current_track_makes_the_index_follow_it() {
    let mut q = queue_of(4, 1);
    assert!(q.move_item(1, 3));
    assert_eq!(order(&q), "t1 t3 t4 t2");
    assert_eq!(q.index(), Some(3));
    assert_eq!(q.current().unwrap().id, "t2");
    assert_eq!(q.hand_len(), 0);
    assert_eq!(q.auto_next_index(), None, "nothing follows the track that was moved to the end");
}

#[test]
fn the_next_track_follows_a_move_right_after_the_current_one() {
    let mut q = queue_of(5, 0);
    assert!(q.move_item(3, 1));
    assert_eq!(order(&q), "t1 t4 t2 t3 t5");
    assert_eq!(q.auto_next_index(), Some(1));
    assert_eq!(q.tracks()[1].id, "t4");
}

#[test]
fn dropping_inside_the_hand_line_joins_it_and_leaving_it_shrinks_it() {
    let mut q = queue_of(6, 0); // t1 playing; t2..t6 follow
    q.enqueue(vec![mk_track("h1", "B", 200.0), mk_track("h2", "B", 200.0)]);
    assert_eq!(order(&q), "t1 h1 h2 t2 t3 t4 t5 t6");
    assert_eq!(q.hand_len(), 2);

    assert!(q.move_item(5, 2), "an ordinary track dropped between h1 and h2");
    assert_eq!(order(&q), "t1 h1 t4 h2 t2 t3 t5 t6");
    assert_eq!(q.hand_len(), 3, "it joined the line");

    assert!(q.move_item(1, 6), "a line track dragged far away");
    assert_eq!(order(&q), "t1 t4 h2 t2 t3 t5 h1 t6");
    assert_eq!(q.hand_len(), 2, "the line shrank");

    assert!(q.move_item(2, 1), "reordering inside the line keeps its length");
    assert_eq!(order(&q), "t1 h2 t4 t2 t3 t5 h1 t6");
    assert_eq!(q.hand_len(), 2);

    assert!(q.move_item(1, 2), "a line track dragged DOWN onto the last slot of the line stays in it");
    assert_eq!(order(&q), "t1 t4 h2 t2 t3 t5 h1 t6");
    assert_eq!(q.hand_len(), 2);
    assert!(q.move_item(2, 1), "and back up");
    assert_eq!(order(&q), "t1 h2 t4 t2 t3 t5 h1 t6");
    assert_eq!(q.hand_len(), 2);

    assert!(q.move_item(7, 3), "dropped right after the line is not part of it");
    assert_eq!(order(&q), "t1 h2 t4 t6 t2 t3 t5 h1");
    assert_eq!(q.hand_len(), 2);
}

#[test]
fn moves_never_touch_the_queue_from_before_shuffle() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_queue(lib[0].tracks.clone(), Some(0));
    q.set_shuffle(true);
    let before = q.to_session(0.0).2;
    assert!(q.move_item(3, 1));
    assert_eq!(q.to_session(0.0).2, before);
    assert_eq!(q.index(), Some(0));
    q.set_shuffle(false);
    let album: Vec<String> = lib[0].tracks.iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids(&q), album, "back to the album order, whatever was dragged around meanwhile");
}

#[test]
fn shuffle_keeps_queue_scoped_to_album_and_restores_order() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_queue(lib[0].tracks.clone(), Some(1));
    assert_eq!(q.current().unwrap().id, "/a/2");

    q.set_shuffle(true);
    assert_eq!(q.index(), Some(0));
    assert_eq!(q.current().unwrap().id, "/a/2");
    // Scoped to Album A only: never pulls in Album B.
    let mut all = ids(&q);
    assert_eq!(all.len(), 4);
    all.sort();
    assert_eq!(all, vec!["/a/1", "/a/2", "/a/3", "/a/4"]);

    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/a/1", "/a/2", "/a/3", "/a/4"]);
    assert_eq!(q.index(), Some(1));
}

#[test]
fn play_in_shuffle_with_none_picks_random_start_within_album() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_shuffle(true);
    q.set_queue(lib[1].tracks.clone(), None);
    assert_eq!(q.index(), Some(0));
    assert_eq!(q.tracks().len(), 3);
    let started_id = q.current().unwrap().id.clone();
    assert!(started_id.starts_with("/b/"), "{started_id}");

    let mut all = ids(&q);
    all.sort();
    assert_eq!(all, vec!["/b/1", "/b/2", "/b/3"]);

    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/b/1", "/b/2", "/b/3"]);
    assert_eq!(q.current().unwrap().id, started_id);
}

#[test]
fn play_in_shuffle_with_explicit_index_puts_chosen_track_first() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_shuffle(true);
    q.set_queue(lib[1].tracks.clone(), Some(2));
    assert_eq!(q.current().unwrap().id, "/b/3");
    assert_eq!(q.tracks().len(), 3);
    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/b/1", "/b/2", "/b/3"]);
    assert_eq!(q.index(), Some(2));
}

#[test]
fn restored_shuffled_session_keeps_active_order_and_unshuffled_restores_when_off() {
    let lib = library();
    let index = build_track_index(&lib);
    let lookup = TrackLookup::new(&lib, &index);
    let state = SessionState {
        current_id: Some("/a/3".to_owned()),
        index: 0,
        position: 0.0,
        shuffle: true,
        repeat_mode: 0,
    };
    let saved: Vec<String> = ["/a/3", "/a/1", "/a/4", "/a/2"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let orig: Vec<String> = ["/a/1", "/a/2", "/a/3", "/a/4"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut q = PlaybackQueue::default();
    q.restore_session(&lookup, &state, &saved, Some(&orig));
    assert_eq!(ids(&q), saved.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(q.current().unwrap().id, "/a/3");
    let (_, active, original) = q.to_session(12.0);
    assert_eq!(active, saved);
    assert_eq!(original, orig);
    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/a/1", "/a/2", "/a/3", "/a/4"]);
    assert_eq!(q.index(), Some(2));
    // Not shuffling: no queue from before is saved.
    assert!(q.to_session(0.0).2.is_empty());

    // Without saved original queue: turning shuffle off keeps the active tracks as-is.
    let mut q = PlaybackQueue::default();
    q.restore_session(&lookup, &state, &saved, None);
    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/a/3", "/a/1", "/a/4", "/a/2"]);
}

#[test]
fn hand_queue_add_appends_to_line_and_play_next_goes_first() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_queue(lib[0].tracks.clone(), Some(0));
    assert!(!q.enqueue(vec![mk_track("/x/1", "X", 200.0)]));
    assert!(!q.enqueue(vec![mk_track("/x/2", "X", 200.0)]));
    // ADD_TO_QUEUE lands right after the current track, not after the album.
    assert_eq!(upcoming(&q), vec!["/x/1", "/x/2", "/a/2", "/a/3", "/a/4"]);
    assert_eq!(q.hand_len(), 2);

    assert!(!q.play_next(vec![mk_track("/y/1", "Y", 200.0)]));
    assert_eq!(upcoming(&q), vec!["/y/1", "/x/1", "/x/2", "/a/2", "/a/3", "/a/4"]);
    assert_eq!(q.hand_len(), 3);

    // Playing on in order uses the line up one track at a time.
    q.step_next(false);
    assert_eq!(q.current().unwrap().id, "/y/1");
    assert_eq!(q.hand_len(), 2);
    q.enqueue(vec![mk_track("/x/3", "X", 200.0)]);
    assert_eq!(upcoming(&q), vec!["/x/1", "/x/2", "/x/3", "/a/2", "/a/3", "/a/4"]);

    // A jump out of order ends the line.
    let a3 = q.tracks().iter().position(|t| t.id == "/a/3").unwrap();
    q.jump_to(a3);
    assert_eq!(q.hand_len(), 0);
    q.enqueue(vec![mk_track("/z/1", "Z", 200.0)]);
    assert_eq!(upcoming(&q), vec!["/z/1", "/a/4"]);
}

#[test]
fn hand_queue_in_shuffle_also_lands_in_the_queue_from_before() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_queue(lib[0].tracks.clone(), Some(0));
    q.set_shuffle(true);
    q.enqueue(vec![mk_track("/x/1", "X", 200.0)]);
    q.play_next(vec![mk_track("/y/1", "Y", 200.0)]);
    assert_eq!(&upcoming(&q)[..2], &["/y/1", "/x/1"]);
    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/a/1", "/y/1", "/x/1", "/a/2", "/a/3", "/a/4"]);
}

#[test]
fn enqueue_into_empty_queue_starts_a_new_one() {
    let mut q = PlaybackQueue::default();
    assert!(q.enqueue(vec![mk_track("/x/1", "X", 200.0)]));
    assert_eq!(q.current().unwrap().id, "/x/1");
    assert_eq!(q.hand_len(), 0);
}

#[test]
fn remove_at_handles_duplicate_ids_and_hand_line() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    let dup = mk_track("/d/1", "D", 200.0);
    q.set_queue(
        vec![mk_track("/a/1", "A", 200.0), dup.clone(), mk_track("/a/2", "A", 200.0), dup.clone()],
        Some(0),
    );
    // Removing the second copy keeps the first one where it is.
    assert!(q.remove_at(3));
    assert_eq!(ids(&q), vec!["/a/1", "/d/1", "/a/2"]);

    // In shuffle, the queue from before keeps as many copies as the play order still has.
    let mut q = PlaybackQueue::default();
    q.set_queue(
        vec![mk_track("/a/1", "A", 200.0), dup.clone(), mk_track("/a/2", "A", 200.0), dup.clone()],
        Some(0),
    );
    q.set_shuffle(true);
    let first_dup = q.tracks().iter().position(|t| t.id == "/d/1").unwrap();
    assert!(q.remove_at(first_dup));
    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/a/1", "/d/1", "/a/2"]);

    // Removing a hand-queued track shortens the line.
    let mut q = PlaybackQueue::default();
    q.set_queue(lib[0].tracks.clone(), Some(0));
    q.enqueue(vec![mk_track("/x/1", "X", 200.0), mk_track("/x/2", "X", 200.0)]);
    assert!(q.remove_at(1));
    assert_eq!(q.hand_len(), 1);
    q.enqueue(vec![mk_track("/x/3", "X", 200.0)]);
    assert_eq!(upcoming(&q), vec!["/x/2", "/x/3", "/a/2", "/a/3", "/a/4"]);
    assert!(!q.remove_at(99));
}

#[test]
fn repeat_modes_handle_wrap_and_single_loop() {
    let mut q = PlaybackQueue::default();
    let tracks = vec![
        mk_track("/m/1.flac", "A", 100.0),
        mk_track("/m/2.flac", "B", 100.0),
    ];
    q.set_queue(tracks, Some(1));
    assert_eq!(q.auto_next_index(), None);
    assert!(q.step_next(true).is_none());

    q.set_repeat(RepeatMode::All);
    assert_eq!(q.auto_next_index(), Some(0));
    assert_eq!(q.step_next(true).unwrap().id, "/m/1.flac");

    // Repeat one: the same track follows itself (preloaded for gapless).
    q.set_repeat(RepeatMode::One);
    assert_eq!(q.auto_next_index(), Some(0));
    assert_eq!(q.step_next(true).unwrap().id, "/m/1.flac");
    assert!(matches!(
        q.on_player_advanced("/m/1.flac", Some(0)),
        PlayerAdvance::Followed(t) if t.id == "/m/1.flac"
    ));
    assert_eq!(q.index(), Some(0));
    assert!(!q.crossfade_applies(0, 5, false));
}

#[test]
fn play_tracker_requires_real_playback_and_ignores_seeks() {
    let mut tracker = PlayTracker::default();
    tracker.reset(Some("/m/song.flac".to_owned()));
    // Jump from 0s -> 150s via seek must NOT trigger play count immediately.
    assert_eq!(tracker.on_time_pos(0.0, 180.0, true), None);
    assert_eq!(tracker.on_time_pos(150.0, 180.0, true), None);

    // Threshold for a 180 s track is 90 s; feed one-second ticks.
    let mut fired = None;
    for sec in 1..=92 {
        if let Some(id) = tracker.on_time_pos(150.0 + sec as f64, 180.0, true) {
            fired = Some(id);
        }
    }
    assert_eq!(fired.as_deref(), Some("/m/song.flac"));
    // Subsequent ticks on the same play must not double count.
    assert_eq!(tracker.on_time_pos(245.0, 180.0, true), None);
}

#[test]
fn play_tracker_counts_short_track_heard_in_full() {
    // A 10 s track has to be heard in full (C# threshold 10 s); polls never land on the end.
    for step in [0.2, 0.5, 1.0] {
        let mut tracker = PlayTracker::default();
        tracker.reset(Some("/m/short.flac".to_owned()));
        let mut fired = false;
        let mut t = 0.3_f64.min(step);
        while t < 10.0 {
            fired |= tracker.on_time_pos(t, 10.0, true).is_some();
            t += step;
        }
        assert!(fired, "poll step {step}");
    }
    // Half of it is not enough.
    let mut tracker = PlayTracker::default();
    tracker.reset(Some("/m/short.flac".to_owned()));
    let mut t = 0.0;
    while t < 5.0 {
        assert!(tracker.on_time_pos(t, 10.0, true).is_none());
        t += 0.5;
    }
}

#[test]
fn crossfade_rule_matches_csharp() {
    let a1 = mk_track("/m/1.flac", "Same Album", 180.0);
    let a2 = mk_track("/m/2.flac", "Same Album", 180.0);
    let a2_case = mk_track("/m/2b.flac", "same album", 180.0);
    let b1 = mk_track("/m/3.flac", "Other Album", 180.0);
    let no_album = mk_track("/m/4.flac", "", 180.0);
    let short = mk_track("/m/5.flac", "Short", 11.0);
    let edge = mk_track("/m/6.flac", "Edge", 12.0);
    let unknown = mk_track("/m/7.flac", "Unknown", 0.0);
    let f = CrossfadeMath::should_crossfade;

    // Same album in order stays gapless; in shuffle it fades.
    assert!(!f(5, RepeatMode::Off, false, false, &a1, &a2));
    assert!(f(5, RepeatMode::Off, false, true, &a1, &a2));
    // The album compare is exact, like C# string ==.
    assert!(f(5, RepeatMode::Off, false, false, &a1, &a2_case));
    // An empty album never counts as "same album".
    assert!(f(5, RepeatMode::Off, false, false, &no_album, &no_album));
    assert!(f(5, RepeatMode::Off, false, false, &a1, &b1));
    // Off, repeat one, direct output.
    assert!(!f(0, RepeatMode::Off, false, false, &a1, &b1));
    assert!(!f(5, RepeatMode::One, false, false, &a1, &b1));
    assert!(!f(5, RepeatMode::Off, true, false, &a1, &b1));
    // The current track must last at least 2 * fade + 2 s; unknown length fades.
    assert!(!f(5, RepeatMode::Off, false, false, &short, &b1));
    assert!(f(5, RepeatMode::Off, false, false, &edge, &b1));
    assert!(f(5, RepeatMode::Off, false, false, &unknown, &b1));

    let mut q = PlaybackQueue::default();
    q.set_queue(vec![a1.clone(), b1.clone()], Some(0));
    assert!(q.crossfade_applies(1, 5, false));
    assert!(!q.crossfade_applies(0, 5, false));
    assert!(!q.crossfade_applies(7, 5, false));
}

#[test]
fn equal_power_gains_endpoints() {
    let (out0, in0) = CrossfadeMath::equal_power_gains(0.0, 0.8);
    assert!((out0 - 0.8).abs() < 1e-6);
    assert!(in0.abs() < 1e-6);

    let (out1, in1) = CrossfadeMath::equal_power_gains(1.0, 0.8);
    assert!(out1.abs() < 1e-4);
    assert!((in1 - 0.8).abs() < 1e-6);
}

#[test]
fn enqueue_racing_player_advance_reloads_the_hand_queued_track() {
    // D-25: A plays, B was handed to mpv as next. C is queued by hand just as mpv moves into B.
    let mut q = PlaybackQueue::default();
    let tracks = vec![mk_track("A", "X", 100.0), mk_track("B", "X", 100.0)];
    q.set_queue(tracks, Some(0));
    let handed = q.auto_next_index();
    assert_eq!(handed, Some(1));
    q.enqueue(vec![mk_track("C", "Y", 100.0)]);
    // The controller hands C over now, but mpv already entered B (handed before the edit).
    let handed_after = q.auto_next_index();
    assert_eq!(handed_after, Some(1));
    match q.on_player_advanced("B", handed_after) {
        PlayerAdvance::Reload(t) => assert_eq!(t.id, "C"),
        other => panic!("expected a reload of C, got {other:?}"),
    }
    // C is current and its hand slot used up; B still follows.
    assert_eq!(q.current().unwrap().id, "C");
    assert_eq!(q.hand_len(), 0);
    assert_eq!(q.tracks()[q.auto_next_index().unwrap()].id, "B");

    // The normal case: mpv entered exactly what it was handed.
    match q.on_player_advanced("B", q.auto_next_index()) {
        PlayerAdvance::Followed(t) => assert_eq!(t.id, "B"),
        other => panic!("expected B to follow, got {other:?}"),
    }
    // Last track, nothing handed, repeat off: mpv moving on means stop.
    assert!(matches!(q.on_player_advanced("Z", None), PlayerAdvance::Stop));
}

#[test]
fn gapless_wrap_of_a_repeating_shuffled_queue_deals_a_new_round() {
    let lib = library();
    let mut q = PlaybackQueue::default();
    q.set_queue(lib[0].tracks.clone(), Some(0));
    q.set_shuffle(true);
    q.set_repeat(RepeatMode::All);
    let all_ids = |q: &PlaybackQueue| {
        let mut v: Vec<String> = q.tracks().iter().map(|t| t.id.clone()).collect();
        v.sort();
        v
    };
    let before = all_ids(&q);
    let last = q.tracks().len() - 1;
    q.set_index_for_test(last);
    let first_path = q.tracks()[0].file_path.clone();
    // mpv wrapped into the slot-0 file it was handed: that track keeps playing, the rest is re-dealt.
    match q.on_player_advanced(&first_path, q.auto_next_index()) {
        PlayerAdvance::Followed(t) => assert_eq!(t.file_path, first_path),
        other => panic!("expected Followed, got {other:?}"),
    }
    assert_eq!(q.index(), Some(0));
    assert_eq!(q.tracks()[0].file_path, first_path);
    assert_eq!(all_ids(&q), before, "nothing is lost or duplicated");

    // Without shuffle the order is untouched on a wrap.
    let mut plain = PlaybackQueue::default();
    plain.set_queue(lib[0].tracks.clone(), Some(3));
    plain.set_repeat(RepeatMode::All);
    let order_before = ids(&plain).join(",");
    plain.on_player_advanced("/a/1", plain.auto_next_index());
    assert_eq!(ids(&plain).join(","), order_before);
}

#[test]
fn remove_tracks_by_id_updates_indices_and_stops_when_current_deleted() {
    let mut q = PlaybackQueue::default();
    let tracks: Vec<Track> = (0..5).map(|i| mk_track(&format!("t{i}"), "X", 100.0)).collect();
    q.set_queue(tracks, Some(1));
    q.enqueue(vec![mk_track("t3", "Y", 100.0), mk_track("h2", "Y", 100.0)]);
    // Remove t0 (before current t1) and t3 (in hand queue) -> t1 stays current at index 0.
    let mut drop = HashSet::new();
    drop.insert("t0".to_owned());
    drop.insert("t3".to_owned());
    let (changed, cur_removed) = q.remove_tracks_by_id(&drop);
    assert!(changed);
    assert!(!cur_removed);
    assert_eq!(q.index(), Some(0));
    assert_eq!(q.current().unwrap().id, "t1");
    assert_eq!(q.hand_len(), 1);
    assert_eq!(q.tracks()[1].id, "h2");

    let mut drop_cur = HashSet::new();
    drop_cur.insert("t1".to_owned());
    let (changed2, cur_removed2) = q.remove_tracks_by_id(&drop_cur);
    assert!(changed2);
    assert!(cur_removed2);
    assert_eq!(q.index(), None);
}

#[test]
fn all_track_ids_includes_tracks_only_in_unshuffled_queue() {
    // A restored shuffled session whose active queue lost "/a/2" (e.g. edited before the restart)
    // while the queue from before shuffle still holds it.
    let lib = library();
    let index = build_track_index(&lib);
    let lookup = TrackLookup::new(&lib, &index);
    let state = SessionState {
        current_id: Some("/a/3".to_owned()),
        index: 0,
        position: 0.0,
        shuffle: true,
        repeat_mode: 0,
    };
    let active: Vec<String> = ["/a/3", "/a/1"].iter().map(|s| (*s).to_owned()).collect();
    let orig: Vec<String> = ["/a/1", "/a/2", "/a/3"].iter().map(|s| (*s).to_owned()).collect();
    let mut q = PlaybackQueue::default();
    q.restore_session(&lookup, &state, &active, Some(&orig));
    assert!(q.tracks().iter().all(|t| t.id != "/a/2"), "/a/2 is not in the active queue");

    let all_before: HashSet<&str> = q.all_track_ids().collect();
    assert_eq!(all_before, HashSet::from(["/a/1", "/a/2", "/a/3"]));

    // Removing a track that only the unshuffled queue holds still counts as a change.
    let drop_set = HashSet::from(["/a/2".to_owned()]);
    let (changed, current_removed) = q.remove_tracks_by_id(&drop_set);
    assert!(changed);
    assert!(!current_removed);
    let all_after: HashSet<&str> = q.all_track_ids().collect();
    assert_eq!(all_after, HashSet::from(["/a/1", "/a/3"]));
    q.set_shuffle(false);
    assert_eq!(ids(&q), vec!["/a/1", "/a/3"]);
}

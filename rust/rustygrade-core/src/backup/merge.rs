use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::ANDROID_EXTRA_KEYS;
use super::cover_names::dedupe_preserving_order;
use crate::collections::{PlaylistData, SmartPlaylistData};
use crate::settings::{
    AppSettings, canonical_library_sort, normalize_library_view, normalize_replay_gain,
    sanitize_eq_gains,
};
use crate::stats::PlayStat;

pub(super) fn merge_playlists(
    existing: &mut Vec<PlaylistData>,
    imported: Vec<PlaylistData>,
    old_replaced_covers: &mut Vec<String>,
) {
    let mut used_ids: HashSet<i64> = existing.iter().map(|p| p.id).collect();
    for mut imp in imported {
        if imp.id != 0
            && let Some(ex) = existing.iter_mut().find(|p| {
                p.id == imp.id
                    && (p.name.trim().eq_ignore_ascii_case(imp.name.trim())
                        || p.name.trim().is_empty())
            })
        {
            ex.track_ids = dedupe_preserving_order(
                ex.track_ids.iter().cloned().chain(imp.track_ids),
            );
            if ex.name.trim().is_empty() && !imp.name.trim().is_empty() {
                ex.name = imp.name;
            }
            if let Some(new_cover) = imp.cover_art
                && let Some(prev) = ex.cover_art.replace(new_cover.clone())
                && prev != new_cover
            {
                old_replaced_covers.push(prev);
            }
            for (k, v) in imp.extra {
                ex.extra.insert(k, v);
            }
            continue;
        }
        imp.id = claim_id(&mut used_ids, imp.id);
        existing.push(imp);
    }
}

/// Reserves a free id at or after `wanted` (at least 1). Wraps to 1 instead of overflowing when an
/// imported id sits at `i64::MAX`.
pub(super) fn claim_id(used: &mut HashSet<i64>, wanted: i64) -> i64 {
    let mut id = wanted.max(1);
    while !used.insert(id) {
        id = id.checked_add(1).unwrap_or(1);
    }
    id
}

pub(super) fn merge_smart_playlists(existing: &mut Vec<SmartPlaylistData>, imported: Vec<SmartPlaylistData>) {
    let mut used_ids: HashSet<i64> = existing.iter().map(|s| s.id).collect();
    for mut imp in imported {
        if imp.id != 0
            && let Some(ex) = existing.iter_mut().find(|s| {
                s.id == imp.id
                    && (s.name.trim().eq_ignore_ascii_case(imp.name.trim())
                        || s.name.trim().is_empty())
            })
        {
            *ex = imp;
            continue;
        }
        imp.id = claim_id(&mut used_ids, imp.id);
        existing.push(imp);
    }
}

pub(super) fn build_stats_ci_index(map: &HashMap<String, PlayStat>) -> HashMap<String, String> {
    let mut index = HashMap::with_capacity(map.len());
    for k in map.keys() {
        index.entry(k.to_lowercase()).or_insert_with(|| k.clone());
    }
    index
}

pub(super) fn merge_stat_entry_indexed(
    map: &mut HashMap<String, PlayStat>,
    ci_index: &mut HashMap<String, String>,
    dst_id: String,
    incoming: PlayStat,
) {
    if let Some(entry) = map.get_mut(&dst_id) {
        entry.count = entry.count.max(incoming.count);
        entry.last = entry.last.max(incoming.last);
        return;
    }
    let low = dst_id.to_lowercase();
    if let Some(existing_key) = ci_index.get(&low)
        && let Some(entry) = map.get_mut(existing_key)
    {
        entry.count = entry.count.max(incoming.count);
        entry.last = entry.last.max(incoming.last);
        return;
    }
    ci_index.insert(low, dst_id.clone());
    map.insert(dst_id, incoming);
}

pub(super) fn merge_settings(dst: &mut AppSettings, src: AppSettings) {
    dst.volume = if src.volume.is_finite() {
        src.volume.clamp(0.0, 1.0)
    } else {
        dst.volume
    };
    dst.library_sort = canonical_library_sort(&src.library_sort).to_owned();
    dst.replay_gain = normalize_replay_gain(&src.replay_gain).to_owned();
    // Keep current device's local library_folders (desktop paths do not exist on Android).
    dst.crossfade_seconds = src.crossfade_seconds.clamp(0, 12);
    dst.eq_enabled = src.eq_enabled;
    dst.eq_preset = src.eq_preset;
    dst.eq_gains = sanitize_eq_gains(src.eq_gains);
    dst.scan_on_startup = src.scan_on_startup;
    dst.auto_replay_gain = src.auto_replay_gain;
    dst.accent_from_cover = src.accent_from_cover;
    dst.library_view = normalize_library_view(&src.library_view).to_owned();
    dst.artwork_itunes = src.artwork_itunes;
    dst.artwork_deezer = src.artwork_deezer;
    dst.artwork_musicbrainz = src.artwork_musicbrainz;

    // Preserve Android device-local settings keys in `extra`; copy all other keys (desktop-only
    // settings such as the audio device, tray or Discord options live there untyped).
    let preserved_android: Vec<(String, Value)> = ANDROID_EXTRA_KEYS
        .iter()
        .filter_map(|k| dst.extra.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    for (k, v) in src.extra {
        if !ANDROID_EXTRA_KEYS.contains(&k.as_str()) {
            dst.extra.insert(k, v);
        }
    }
    for (k, v) in preserved_android {
        dst.extra.insert(k, v);
    }
}

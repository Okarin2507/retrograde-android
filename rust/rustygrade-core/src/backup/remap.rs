use serde_json::Value;

use super::cover_names::dedupe_preserving_order;
use super::merge::{build_stats_ci_index, merge_stat_entry_indexed};
use super::path_mapper::{MapOutcome, TrackPathMapper};
use super::{KEY_BACKUP_SOURCE_ROOTS, MAX_SAVED_SOURCE_ROOTS};
use crate::collections::CollectionsData;
use crate::settings::AppSettings;
use crate::stats::{PlayStat, PlayStats};

/// Remaps any non-exact (previously unmatched foreign/desktop) track IDs in `collections`
/// against the current local library. Returns how many references changed.
pub fn remap_stored_collections(
    collections: &mut CollectionsData,
    mapper: &TrackPathMapper,
) -> usize {
    let mut remapped = 0usize;

    let mut changed_liked = false;
    let new_liked: Vec<String> = collections
        .liked
        .iter()
        .map(|id| {
            if mapper.is_exact(id) {
                id.clone()
            } else if let MapOutcome::Remapped(local) = mapper.map_id(id) {
                changed_liked = true;
                remapped += 1;
                local
            } else {
                id.clone()
            }
        })
        .collect();
    if changed_liked {
        collections.liked = dedupe_preserving_order(new_liked.into_iter());
    }

    for p in &mut collections.playlists {
        let mut changed_pl = false;
        let new_ids: Vec<String> = p
            .track_ids
            .iter()
            .map(|id| {
                if mapper.is_exact(id) {
                    id.clone()
                } else if let MapOutcome::Remapped(local) = mapper.map_id(id) {
                    changed_pl = true;
                    remapped += 1;
                    local
                } else {
                    id.clone()
                }
            })
            .collect();
        if changed_pl {
            p.track_ids = dedupe_preserving_order(new_ids.into_iter());
        }
    }

    remapped
}

/// Remaps any non-exact track IDs in `stats` against the current local library in O(n).
/// Returns how many stat keys were remapped.
pub fn remap_stored_stats(stats: &mut PlayStats, mapper: &TrackPathMapper) -> usize {
    let to_remap: Vec<(String, String)> = stats
        .tracks
        .keys()
        .filter(|id| !mapper.is_exact(id))
        .filter_map(|id| match mapper.map_id(id) {
            MapOutcome::Remapped(local) => Some((id.clone(), local)),
            _ => None,
        })
        .collect();

    if to_remap.is_empty() {
        return 0;
    }

    let count = to_remap.len();
    let mut moved: Vec<(String, PlayStat)> = Vec::with_capacity(count);
    for (old_key, new_key) in to_remap {
        if let Some(stat) = stats.tracks.remove(&old_key) {
            moved.push((new_key, stat));
        }
    }
    let mut ci_index = build_stats_ci_index(&stats.tracks);
    for (new_key, stat) in moved {
        merge_stat_entry_indexed(&mut stats.tracks, &mut ci_index, new_key, stat);
    }
    stats.invalidate_index();
    count
}

pub fn saved_backup_source_roots(settings: &AppSettings) -> Vec<String> {
    settings
        .extra
        .get(KEY_BACKUP_SOURCE_ROOTS)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn append_backup_source_roots(settings: &mut AppSettings, new_roots: &[String]) {
    let mut roots = saved_backup_source_roots(settings);
    for r in new_roots {
        let trimmed = r.trim();
        if !trimmed.is_empty() && !roots.iter().any(|existing| existing == trimmed) {
            roots.push(trimmed.to_owned());
        }
    }
    if roots.len() > MAX_SAVED_SOURCE_ROOTS {
        let drop_n = roots.len() - MAX_SAVED_SOURCE_ROOTS;
        roots.drain(..drop_n);
    }
    settings.extra.insert(
        KEY_BACKUP_SOURCE_ROOTS.to_owned(),
        Value::Array(roots.into_iter().map(Value::String).collect()),
    );
}

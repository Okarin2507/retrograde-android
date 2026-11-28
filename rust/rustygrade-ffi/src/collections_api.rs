//! Playlists, favorites and smart playlists for the Android UI, plus cover palettes.
//! Every write is a locked read-modify-write of `native-collections.json`, so the UI and the
//! media service can never lose each other's edits.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rustygrade_core::settings::{KEY_USE_MEDIA_STORE, normalize_library_view};
use rustygrade_core::{
    CollectionsData, CollectionsStore, CoverPaths, PlayStats, PlaylistCovers, PlaylistData,
    SmartPlaylistData, SmartPlaylists, SmartRule, delete_unreferenced_covers_with_extra,
    extract_cover_palette, library::LibraryMap, now_ms,
};

use crate::dto::{PlaylistInfo, SmartPlaylistInfo, TrackInfo};
use crate::engine::RustygradeEngine;
use crate::error::EngineError;

const PREVIEW_TRACKS: usize = 50;

#[derive(Debug, Clone, uniffi::Record)]
pub struct SmartPreview {
    /// How many tracks match, after the playlist's own `limit`.
    pub count: u32,
    /// The first matches, for a live preview while editing rules.
    pub tracks: Vec<TrackInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SmartOpInfo {
    pub key: String,
    pub label: String,
    /// `false` for ops without an argument (`never`, `yes`, `no`).
    pub needs_value: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SmartFieldInfo {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub ops: Vec<SmartOpInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SortOption {
    pub key: String,
    pub label: String,
}

/// Colors as `0xRRGGBB`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaletteInfo {
    pub accent: u32,
    pub glow: u32,
    /// False for grayscale artwork / the fallback.
    pub vivid: bool,
}

fn rgb(c: [u8; 3]) -> u32 {
    (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2])
}

/// Accent and glow colors of a cover file (Orbit orange for unreadable or colorless art).
#[uniffi::export]
pub fn cover_palette(path: String) -> PaletteInfo {
    let p = extract_cover_palette(Path::new(&path)).unwrap_or_default();
    PaletteInfo { accent: rgb(p.accent), glow: rgb(p.glow), vivid: p.vivid }
}

#[uniffi::export]
pub fn smart_fields() -> Vec<SmartFieldInfo> {
    SmartPlaylists::FIELDS
        .iter()
        .map(|f| SmartFieldInfo {
            key: f.key.to_owned(),
            label: f.label.to_owned(),
            hint: f.hint.to_owned(),
            ops: f
                .ops
                .iter()
                .map(|op| SmartOpInfo {
                    key: (*op).to_owned(),
                    label: SmartPlaylists::op_label(op).to_owned(),
                    needs_value: SmartPlaylists::needs_value(op),
                })
                .collect(),
        })
        .collect()
}

/// The 10 library sorts in menu order, with the labels of the desktop build.
#[uniffi::export]
pub fn library_sorts() -> Vec<SortOption> {
    rustygrade_core::LibrarySort::ALL
        .iter()
        .map(|s| SortOption { key: s.as_str().to_owned(), label: s.label().to_owned() })
        .collect()
}

#[uniffi::export]
pub fn smart_sorts() -> Vec<SortOption> {
    SmartPlaylists::SORTS
        .iter()
        .map(|(k, l)| SortOption { key: (*k).to_owned(), label: (*l).to_owned() })
        .collect()
}

fn playlist_info(p: PlaylistData) -> PlaylistInfo {
    PlaylistInfo {
        id: p.id,
        name: p.name,
        cover_path: CoverPaths::to_local_path(p.cover_art.as_deref())
            .map(|p| p.to_string_lossy().into_owned()),
        track_ids: p.track_ids,
    }
}

fn smart_data(p: &SmartPlaylistInfo) -> SmartPlaylistData {
    SmartPlaylistData {
        id: p.id,
        name: p.name.clone(),
        match_all: p.match_all,
        rules: p
            .rules
            .iter()
            .map(|r| SmartRule { field: r.field.clone(), op: r.op.clone(), value: r.value.clone() })
            .collect(),
        sort: p.sort.clone(),
        limit: p.limit as usize,
        ..SmartPlaylistData::default()
    }
}

/// An id no playlist or smart playlist uses yet (milliseconds like the desktop build, bumped on clash).
pub(crate) fn fresh_id(data: &CollectionsData) -> i64 {
    let mut id = now_ms();
    let used = |id: i64| {
        data.playlists.iter().any(|p| p.id == id)
            || data.smart.iter().flatten().any(|s| s.id == id)
    };
    while used(id) {
        // Wraps instead of overflowing when a stored id sits at i64::MAX.
        id = id.checked_add(1).unwrap_or(1);
    }
    id
}

fn reorder_vec(list: &mut Vec<String>, from: u32, to: u32) -> bool {
    let (f, t) = (from as usize, to as usize);
    if f == t || f >= list.len() || t >= list.len() {
        return false;
    }
    let item = list.remove(f);
    list.insert(t, item);
    true
}

#[uniffi::export]
impl RustygradeEngine {
    // ---- favorites ------------------------------------------------------------------------

    pub fn set_liked(&self, track_id: String, liked: bool) {
        self.update_collections(|d| {
            d.liked.retain(|id| id != &track_id);
            if liked {
                d.liked.push(track_id.clone());
            }
        });
    }

    pub fn reorder_liked_track(&self, from_index: u32, to_index: u32) {
        self.update_collections_if(|d| reorder_vec(&mut d.liked, from_index, to_index));
    }

    // ---- playlists ------------------------------------------------------------------------

    pub fn create_playlist(&self, name: String) -> PlaylistInfo {
        let mut created = None;
        self.update_collections(|d| {
            let p = PlaylistData {
                id: fresh_id(d),
                name: name.trim().to_owned(),
                ..PlaylistData::default()
            };
            created = Some(playlist_info(p.clone()));
            d.playlists.push(p);
        });
        created.expect("update_collections always runs its closure")
    }

    pub fn rename_playlist(&self, id: i64, name: String) {
        self.update_collections(|d| {
            if let Some(p) = d.playlists.iter_mut().find(|p| p.id == id) {
                p.name = name.trim().to_owned();
            }
        });
    }

    pub fn delete_playlist(&self, id: i64) {
        let _ = self.update_collections_checked(
            |d| d.playlists.iter().position(|p| p.id == id).map(|at| d.playlists.remove(at).cover_art),
            |d, cover| {
                // Only now that the playlist is gone from the file—and while still holding `collections_lock`—
                // delete its cover if no album or remaining playlist shares it.
                if let Some(full) = CoverPaths::to_local_path(cover.as_deref()) {
                    let mut candidates = Vec::with_capacity(2);
                    let thumb = PlaylistCovers::thumb_for(&full);
                    if thumb != full {
                        candidates.push(thumb);
                    }
                    candidates.push(full);
                    self.delete_unshared_covers(&candidates, d);
                }
            },
        );
    }

    /// Appends tracks the playlist does not hold yet. Returns how many were added.
    pub fn add_to_playlist(&self, id: i64, track_ids: Vec<String>) -> u32 {
        let mut added = 0;
        self.update_collections_if(|d| {
            if let Some(p) = d.playlists.iter_mut().find(|p| p.id == id) {
                let mut have: HashSet<String> = p.track_ids.iter().cloned().collect();
                for t in &track_ids {
                    if have.insert(t.clone()) {
                        p.track_ids.push(t.clone());
                        added += 1;
                    }
                }
            }
            added > 0
        });
        added
    }

    pub fn remove_from_playlist(&self, id: i64, track_id: String) {
        self.update_collections(|d| {
            if let Some(p) = d.playlists.iter_mut().find(|p| p.id == id) {
                p.track_ids.retain(|t| t != &track_id);
            }
        });
    }

    pub fn reorder_playlist_track(&self, id: i64, from_index: u32, to_index: u32) {
        self.update_collections_if(|d| {
            d.playlists
                .iter_mut()
                .find(|p| p.id == id)
                .is_some_and(|p| reorder_vec(&mut p.track_ids, from_index, to_index))
        });
    }

    /// Sets the cover from an image file (jpg/png/webp), or removes it with `None`.
    /// Returns the stored cover path.
    pub fn set_playlist_cover(&self, id: i64, image_path: Option<String>) -> Result<Option<String>, EngineError> {
        // Decoding, resizing and hashing are slow: do them before taking the lock, so
        // playlist and favorite edits are not held up behind an image.
        let prepared = match &image_path {
            Some(src) => Some(
                PlaylistCovers::prepare_from_path(Path::new(src))
                    .map_err(|e| EngineError::Library(e.to_string()))?,
            ),
            None => None,
        };
        let (new_cover, created_new) = match &prepared {
            Some(p) => {
                let (url, created) = PlaylistCovers::save_prepared(p)
                    .map_err(|e| EngineError::Library(e.to_string()))?;
                (Some(url), created)
            }
            None => (None, false),
        };
        let saved = self.update_collections_checked(
            |d| {
                d.playlists
                    .iter_mut()
                    .find(|p| p.id == id)
                    .map(|p| std::mem::replace(&mut p.cover_art, new_cover.clone()))
            },
            |d, previous| {
                // The new state is on disk: delete the old files only if no album or playlist
                // still points at them.
                if previous != new_cover
                    && let Some(full) = CoverPaths::to_local_path(previous.as_deref())
                {
                    let mut candidates = Vec::with_capacity(2);
                    let thumb = PlaylistCovers::thumb_for(&full);
                    if thumb != full {
                        candidates.push(thumb);
                    }
                    candidates.push(full);
                    self.delete_unshared_covers(&candidates, d);
                }
            },
        );
        match saved {
            Ok(true) => Ok(new_cover
                .as_deref()
                .and_then(|url| CoverPaths::to_local_path(Some(url)))
                .map(|p| p.to_string_lossy().into_owned())),
            // No such playlist, or the save failed: discard the file only if we just created it.
            Ok(false) => {
                if created_new {
                    PlaylistCovers::delete(new_cover.as_deref());
                }
                Ok(None)
            }
            Err(e) => {
                if created_new {
                    PlaylistCovers::delete(new_cover.as_deref());
                }
                Err(e)
            }
        }
    }

    pub fn playlist(&self, id: i64) -> Option<PlaylistInfo> {
        CollectionsStore::load().playlists.into_iter().find(|p| p.id == id).map(playlist_info)
    }

    // ---- smart playlists ------------------------------------------------------------------

    /// Creates (`id == 0` or unknown) or replaces a smart playlist. Returns its id.
    pub fn save_smart_playlist(&self, playlist: SmartPlaylistInfo) -> i64 {
        let mut saved = 0;
        self.update_collections(|d| {
            let mut data = smart_data(&playlist);
            data.name = data.name.trim().to_owned();
            let new_id = fresh_id(d);
            let list = d.smart.get_or_insert_with(Vec::new);
            if let Some(existing) = list.iter_mut().find(|s| s.id == data.id && data.id != 0) {
                data.extra = std::mem::take(&mut existing.extra);
                saved = data.id;
                *existing = data;
            } else {
                data.id = new_id;
                saved = new_id;
                list.push(data);
            }
        });
        saved
    }

    pub fn delete_smart_playlist(&self, id: i64) {
        self.update_collections(|d| {
            if let Some(list) = d.smart.as_mut() {
                list.retain(|s| s.id != id);
            }
        });
    }

    /// Live result of a (possibly unsaved) smart playlist: the count and the first matches.
    pub fn smart_preview(&self, playlist: SmartPlaylistInfo) -> Result<SmartPreview, EngineError> {
        let tracks = self.evaluate_smart(&smart_data(&playlist))?;
        Ok(SmartPreview {
            count: tracks.len() as u32,
            tracks: tracks.iter().take(PREVIEW_TRACKS).map(TrackInfo::from).collect(),
        })
    }

    /// All tracks of a saved smart playlist, in its sort order.
    pub fn smart_playlist_tracks(&self, id: i64) -> Result<Vec<TrackInfo>, EngineError> {
        // One read of the collections serves both the playlist definition and the favorites.
        let data = CollectionsStore::load();
        let Some(s) = data.smart.as_deref().unwrap_or_default().iter().find(|s| s.id == id) else {
            return Ok(Vec::new());
        };
        let liked: HashSet<&str> = data.liked.iter().map(String::as_str).collect();
        Ok(self.evaluate_smart_with(s, &liked)?.iter().map(TrackInfo::from).collect())
    }

    // ---- settings the UI owns -------------------------------------------------------------

    pub fn set_library_sort(&self, sort: String) {
        self.update_settings_field(|s| s.library_sort = rustygrade_core::canonical_library_sort(&sort).to_owned());
    }

    /// `"albums"` or `"artists"`.
    pub fn set_library_view(&self, view: String) {
        let view = normalize_library_view(&view).to_owned();
        self.update_settings_field(|s| s.library_view = view);
    }

    pub fn set_accent_from_cover(&self, enabled: bool) {
        self.update_settings_field(|s| s.accent_from_cover = enabled);
    }

    pub fn set_use_media_store(&self, enabled: bool) {
        self.update_settings_field(|s| {
            s.extra.insert(KEY_USE_MEDIA_STORE.to_owned(), serde_json::Value::Bool(enabled));
        });
    }
}

impl RustygradeEngine {
    fn evaluate_smart(&self, playlist: &SmartPlaylistData) -> Result<Vec<rustygrade_core::Track>, EngineError> {
        let collections = CollectionsStore::load();
        let liked: HashSet<&str> = collections.liked.iter().map(String::as_str).collect();
        self.evaluate_smart_with(playlist, &liked)
    }

    fn evaluate_smart_with(
        &self,
        playlist: &SmartPlaylistData,
        liked: &HashSet<&str>,
    ) -> Result<Vec<rustygrade_core::Track>, EngineError> {
        let data = self.library()?;
        let stats = PlayStats::load_locked();
        let entries = data.albums.iter().flat_map(|a| a.tracks.iter().map(move |t| (t, a)));
        Ok(SmartPlaylists::evaluate(playlist, entries, |id| stats.get(id), |id| liked.contains(id), now_ms()))
    }

    /// Deletes the `candidates` (a playlist's old cover files) unless an album of the cached
    /// library or a playlist in `collections` still uses them. Uses the in-memory library instead
    /// of re-parsing `library.json`; if the library cannot be loaded, nothing is deleted.
    fn delete_unshared_covers(&self, candidates: &[PathBuf], collections: &CollectionsData) {
        let Ok(data) = self.library() else {
            return;
        };
        let album_refs: HashSet<PathBuf> = data
            .albums
            .iter()
            .flat_map(|a| [a.cover_thumb_path.clone(), a.cover_full_path.clone()])
            .flatten()
            .collect();
        delete_unreferenced_covers_with_extra(candidates, &LibraryMap::new(), collections, &album_refs);
    }
}

#[cfg(test)]
mod tests {
    use rustygrade_core::testing::{Fixture, TestEnv, write_flac};

    use super::*;
    use crate::dto::{ScanRequest, SmartRuleInfo};

    fn setup(env: &TestEnv) -> std::sync::Arc<RustygradeEngine> {
        write_flac(&env.music.join("A/01.flac"), &Fixture::new("a1", "Alice", "A", 1).with_cover());
        write_flac(&env.music.join("A/02.flac"), &Fixture::new("a2", "Alice", "A", 2));
        write_flac(&env.music.join("B/01.flac"), &Fixture::new("b1", "Bob", "B", 1));
        let lib = RustygradeEngine::new(env.app_dir.to_string_lossy().into_owned());
        let req = ScanRequest { dirs: vec![env.music.to_string_lossy().into_owned()], ..ScanRequest::default() };
        lib.scan_sources(req, None, None).unwrap();
        lib
    }

    fn ids(lib: &RustygradeEngine, album: &str) -> Vec<String> {
        lib.get_album_detail(album.into()).unwrap().unwrap().tracks.into_iter().map(|t| t.id).collect()
    }

    #[test]
    fn playlists_crud_dedupes_and_favorites_toggle() {
        let env = TestEnv::new("api-playlists");
        let lib = setup(&env);
        let a = ids(&lib, "A");
        let p = lib.create_playlist("  Road trip ".into());
        assert_eq!(p.name, "Road trip");
        let q = lib.create_playlist("Gym".into());
        assert_ne!(p.id, q.id, "ids never clash");

        assert_eq!(lib.add_to_playlist(p.id, a.clone()), 2);
        assert_eq!(lib.add_to_playlist(p.id, a.clone()), 0, "no duplicates");
        lib.remove_from_playlist(p.id, a[0].clone());
        lib.rename_playlist(q.id, "Gym 2".into());
        let c = lib.load_collections();
        assert_eq!(c.playlists.iter().find(|x| x.id == p.id).unwrap().track_ids, vec![a[1].clone()]);
        assert_eq!(c.playlists.iter().find(|x| x.id == q.id).unwrap().name, "Gym 2");

        lib.set_liked(a[0].clone(), true);
        lib.set_liked(a[0].clone(), true);
        assert_eq!(lib.load_collections().liked, vec![a[0].clone()], "liked once");
        lib.set_liked(a[0].clone(), false);
        assert!(lib.load_collections().liked.is_empty());

        lib.delete_playlist(p.id);
        assert!(lib.playlist(p.id).is_none() && lib.playlist(q.id).is_some());
    }

    #[test]
    fn concurrent_collection_edits_do_not_lose_updates() {
        let env = TestEnv::new("api-concurrent");
        let lib = setup(&env);
        let id = lib.create_playlist("P".into()).id;
        let all: Vec<String> = ids(&lib, "A").into_iter().chain(ids(&lib, "B")).collect();
        std::thread::scope(|s| {
            for t in &all {
                let (lib, t) = (&lib, t.clone());
                s.spawn(move || {
                    lib.add_to_playlist(id, vec![t.clone()]);
                    lib.set_liked(t, true);
                });
            }
        });
        let c = lib.load_collections();
        assert_eq!(c.playlists[0].track_ids.len(), 3);
        assert_eq!(c.liked.len(), 3);
    }

    #[test]
    fn playlist_cover_is_stored_and_replaced_and_removed() {
        let env = TestEnv::new("api-cover");
        let lib = setup(&env);
        let id = lib.create_playlist("P".into()).id;
        let img1 = env.root.join("pic1.png");
        let img2 = env.root.join("pic2.png");
        rustygrade_core::testing::write_png(&img1, [10, 120, 200]);
        rustygrade_core::testing::write_png(&img2, [210, 40, 80]);

        let first = lib.set_playlist_cover(id, Some(img1.to_string_lossy().into_owned())).unwrap().unwrap();
        assert!(Path::new(&first).exists());
        let second = lib.set_playlist_cover(id, Some(img2.to_string_lossy().into_owned())).unwrap().unwrap();
        assert_ne!(first, second);
        assert!(!Path::new(&first).exists(), "the old cover file is deleted");
        assert!(lib.set_playlist_cover(id, Some("/nope/missing.png".into())).is_err());
        assert_eq!(lib.playlist(id).unwrap().cover_path.as_deref(), Some(second.as_str()), "a failed set keeps the cover");
        lib.set_playlist_cover(id, None).unwrap();
        assert!(lib.playlist(id).unwrap().cover_path.is_none());
        assert!(!Path::new(&second).exists());
    }

    #[test]
    fn smart_playlists_preview_save_and_evaluate() {
        let env = TestEnv::new("api-smart");
        let lib = setup(&env);
        let mut p = SmartPlaylistInfo {
            id: 0,
            name: " Alice only ".into(),
            match_all: true,
            rules: vec![SmartRuleInfo { field: "artist".into(), op: "is".into(), value: "alice".into() }],
            sort: "title".into(),
            limit: 0,
        };
        let preview = lib.smart_preview(p.clone()).unwrap();
        assert_eq!(preview.count, 2);
        assert_eq!(preview.tracks[0].title, "a1");

        // An empty value on an op that needs one is ignored, so the rule matches everything.
        let mut open = p.clone();
        open.rules[0].value = "  ".into();
        assert_eq!(lib.smart_preview(open).unwrap().count, 3);
        p.limit = 1;
        assert_eq!(lib.smart_preview(p.clone()).unwrap().count, 1, "limit applies");
        p.limit = 0;

        let id = lib.save_smart_playlist(p.clone());
        assert_ne!(id, 0);
        let saved = lib.load_collections().smart.into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(saved.name, "Alice only");
        assert_eq!(lib.smart_playlist_tracks(id).unwrap().len(), 2);

        // Editing keeps the id; the match mode and rules change.
        p.id = id;
        p.rules[0].value = "bob".into();
        assert_eq!(lib.save_smart_playlist(p), id);
        assert_eq!(lib.smart_playlist_tracks(id).unwrap()[0].title, "b1");
        let count = lib.load_collections().smart.len();
        lib.delete_smart_playlist(id);
        assert_eq!(lib.load_collections().smart.len(), count - 1);
        assert!(lib.smart_playlist_tracks(id).unwrap().is_empty());
    }

    #[test]
    fn smart_catalog_and_palette() {
        let fields = smart_fields();
        assert!(fields.iter().any(|f| f.key == "liked" && f.ops.iter().all(|o| !o.needs_value)));
        assert!(fields.iter().all(|f| f.ops.iter().all(|o| !o.label.is_empty())));
        assert!(smart_sorts().iter().any(|s| s.key == "random"));
        let sorts = library_sorts();
        assert_eq!(sorts.len(), 10);
        assert_eq!(sorts[0].key, "added_desc");

        let env = TestEnv::new("api-palette");
        let img = env.root.join("red.png");
        rustygrade_core::testing::write_png(&img, [200, 30, 30]);
        let p = cover_palette(img.to_string_lossy().into_owned());
        assert!(p.vivid && (p.accent >> 16) > 0x80, "reddish accent: {:06x}", p.accent);
        let fallback = cover_palette("/nope.png".into());
        assert_eq!(fallback.accent, 0xFF6B35, "Orbit orange when unreadable");
    }

    #[test]
    fn ui_setting_setters_touch_only_their_field() {
        let env = TestEnv::new("api-ui-settings");
        let lib = setup(&env);
        lib.set_library_sort("year_desc".into());
        lib.set_library_view("artists".into());
        lib.set_accent_from_cover(false);
        lib.set_use_media_store(false);
        lib.set_library_view("whatever".into());
        let s = lib.load_settings();
        assert_eq!((s.library_sort.as_str(), s.library_view.as_str()), ("year_desc", "albums"));
        assert!(!s.accent_from_cover && !s.use_media_store);
    }

    /// Makes the app dir read-only so writing `native-collections.json` fails, until the guard drops.
    /// `None` when the process can write anyway (running as root): such a test cannot run.
    struct ReadOnlyAppDir(std::path::PathBuf);

    impl ReadOnlyAppDir {
        fn new(dir: &Path) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).ok()?;
            if std::fs::write(dir.join(".probe"), b"x").is_ok() {
                let _ = std::fs::remove_file(dir.join(".probe"));
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
                return None;
            }
            Some(Self(dir.to_path_buf()))
        }
    }

    impl Drop for ReadOnlyAppDir {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn a_failed_collections_save_keeps_the_old_cover_and_drops_the_new_files() {
        let env = TestEnv::new("api-cover-save-fails");
        let lib = setup(&env);
        let id = lib.create_playlist("P".into()).id;
        let img1 = env.root.join("pic1.png");
        let img2 = env.root.join("pic2.png");
        rustygrade_core::testing::write_png(&img1, [10, 120, 200]);
        rustygrade_core::testing::write_png(&img2, [220, 50, 30]);
        let old = lib.set_playlist_cover(id, Some(img1.to_string_lossy().into_owned())).unwrap().unwrap();
        let covers_before = std::fs::read_dir(CoverPaths::cover_dir()).unwrap().count();

        let Some(_ro) = ReadOnlyAppDir::new(&env.app_dir) else { return };
        let err = lib.set_playlist_cover(id, Some(img2.to_string_lossy().into_owned())).unwrap_err();
        assert!(err.to_string().contains("could not save"), "{err}");
        assert!(Path::new(&old).exists(), "the old cover survives a failed save");
        assert_eq!(lib.playlist(id).unwrap().cover_path.as_deref(), Some(old.as_str()));
        let covers_after = std::fs::read_dir(CoverPaths::cover_dir()).unwrap().count();
        assert_eq!(covers_before, covers_after, "the new files were not left behind as orphans");

        // Removing the cover fails the same way and keeps the file; deleting the playlist too.
        assert!(lib.set_playlist_cover(id, None).is_err());
        assert!(Path::new(&old).exists());
        lib.delete_playlist(id);
        assert!(Path::new(&old).exists(), "the cover of a playlist that is still saved is kept");
        assert!(lib.playlist(id).is_some());
    }

    #[test]
    fn deleting_a_playlist_removes_its_cover_only_after_saving() {
        let env = TestEnv::new("api-delete-cover");
        let lib = setup(&env);
        let id = lib.create_playlist("P".into()).id;
        let img = env.root.join("pic.png");
        rustygrade_core::testing::write_png(&img, [90, 20, 20]);
        let cover = lib.set_playlist_cover(id, Some(img.to_string_lossy().into_owned())).unwrap().unwrap();
        assert!(Path::new(&cover).exists());
        lib.delete_playlist(id);
        assert!(lib.playlist(id).is_none());
        assert!(!Path::new(&cover).exists());
        lib.delete_playlist(id); // unknown id: nothing happens
        // Setting a cover on a playlist that does not exist discards the files it made.
        let before = std::fs::read_dir(CoverPaths::cover_dir()).map(|d| d.count()).unwrap_or(0);
        assert_eq!(lib.set_playlist_cover(999, Some(img.to_string_lossy().into_owned())).unwrap(), None);
        assert_eq!(std::fs::read_dir(CoverPaths::cover_dir()).map(|d| d.count()).unwrap_or(0), before);
    }

    #[test]
    fn a_failed_save_is_logged_for_infallible_calls() {
        let env = TestEnv::new("api-save-logged");
        let lib = setup(&env);
        // A directory where the file belongs: loading finds nothing usable and the final rename fails,
        // while the app dir itself stays writable for the error log.
        let file = env.app_dir.join("native-collections.json");
        let _ = std::fs::remove_file(&file);
        std::fs::create_dir(&file).unwrap();
        // `create_playlist` cannot return an error to Kotlin; the failure must at least be visible.
        let _ = lib.create_playlist("Lost".into());
        let log = std::fs::read_to_string(rustygrade_core::error_log::file_path()).unwrap_or_default();
        assert!(log.contains("could not save native-collections.json"), "{log}");
    }

    #[test]
    fn fresh_ids_skip_used_ones_and_never_overflow() {
        let mut d = CollectionsData::default();
        d.playlists.push(PlaylistData { id: i64::MAX, ..PlaylistData::default() });
        let id = fresh_id(&d);
        assert_ne!(id, i64::MAX);
        let mut d = CollectionsData::default();
        let now = now_ms();
        d.playlists.push(PlaylistData { id: now, ..PlaylistData::default() });
        assert!(fresh_id(&d) >= now);
    }

    #[test]
    fn shared_content_cover_survives_deleting_or_changing_one_playlist() {
        use crate::backup_api::{BackupBundleDto, BackupCoverEntry};

        let env = TestEnv::new("api-shared-cover");
        let lib = setup(&env);
        let img = env.root.join("shared.png");
        rustygrade_core::testing::write_png(&img, [45, 160, 220]);
        let cover_bytes = std::fs::read(&img).unwrap();
        let hash = "abcdef0123456789abcdef01";

        let bundle = BackupBundleDto {
            collections_json: Some(format!(
                r#"{{
                    "playlists": [
                        {{"id": 101, "name": "One", "coverArt": "/home/okarin/.rustygrade/covers/c_{hash}.png"}},
                        {{"id": 102, "name": "Two", "coverArt": "/home/okarin/.rustygrade/covers/c_{hash}.png"}}
                    ]
                }}"#
            )),
            playlist_covers: vec![BackupCoverEntry {
                name: format!("c_{hash}.png"),
                bytes: cover_bytes,
            }],
            ..Default::default()
        };

        let report = lib.import_backup(bundle, false, None).unwrap();
        assert_eq!(report.covers_restored, 1);

        let shared_path = CoverPaths::cover_dir().join(format!("c_{hash}.png"));
        assert!(shared_path.is_file());

        // Removing cover on playlist 101 must keep the file for playlist 102.
        lib.set_playlist_cover(101, None).unwrap();
        assert!(shared_path.is_file());
        assert_eq!(
            lib.playlist(102).unwrap().cover_path.as_deref(),
            Some(shared_path.to_string_lossy().as_ref())
        );

        // Deleting playlist 101 keeps it too; deleting playlist 102 removes the file.
        lib.delete_playlist(101);
        assert!(shared_path.is_file());
        lib.delete_playlist(102);
        assert!(!shared_path.exists());
    }

    #[test]
    fn reordering_playlist_and_liked_tracks_persists_valid_moves_and_ignores_out_of_bounds() {
        let env = TestEnv::new("api-reorder");
        let lib = setup(&env);
        let a_ids = ids(&lib, "A");
        let b_ids = ids(&lib, "B");
        let all = vec![a_ids[0].clone(), a_ids[1].clone(), b_ids[0].clone()];

        for t in &all {
            lib.set_liked(t.clone(), true);
        }
        lib.reorder_liked_track(0, 2);
        lib.reorder_liked_track(1, 1); // no-op
        lib.reorder_liked_track(9, 0); // out of bounds
        assert_eq!(lib.load_collections().liked, vec![all[1].clone(), all[2].clone(), all[0].clone()]);

        let pl = lib.create_playlist("Order".into());
        lib.add_to_playlist(pl.id, all.clone());
        lib.reorder_playlist_track(pl.id, 2, 0);
        lib.reorder_playlist_track(pl.id, 0, 5); // out of bounds
        assert_eq!(
            lib.playlist(pl.id).unwrap().track_ids,
            vec![all[2].clone(), all[0].clone(), all[1].clone()]
        );
    }

    #[test]
    fn deleting_or_clearing_playlist_cover_preserves_file_when_shared_with_album() {
        let env = TestEnv::new("api-shared-album-cover");
        let lib = setup(&env);
        let album_cover = lib
            .get_album_detail("A".into())
            .unwrap()
            .unwrap()
            .album
            .thumb_path
            .expect("Album A has cover");
        let p_id = lib.create_playlist("Shared With Album".into()).id;
        let stored = lib.set_playlist_cover(p_id, Some(album_cover.clone())).unwrap().unwrap();
        assert_eq!(stored, album_cover, "identical normalized cover reuses album file");

        lib.set_playlist_cover(p_id, None).unwrap();
        assert!(Path::new(&album_cover).is_file(), "clearing playlist cover must keep album cover");

        // Deleting the playlist referencing an album cover also preserves the album cover.
        let p2_id = lib.create_playlist("Shared Two".into()).id;
        lib.set_playlist_cover(p2_id, Some(album_cover.clone())).unwrap();
        lib.delete_playlist(p2_id);
        assert!(Path::new(&album_cover).is_file(), "deleting playlist must keep album cover");
    }
}

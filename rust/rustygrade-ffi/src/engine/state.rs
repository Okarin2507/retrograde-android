use std::sync::atomic::Ordering;
use std::sync::Arc;

use rustygrade_core::library::LibraryStore;
use rustygrade_core::settings::KEY_SAF_TREES;
use rustygrade_core::{AppSettings, CollectionsData, CollectionsStore, group_artists};
use serde_json::Value;

use super::{LibraryData, RustygradeEngine};
use crate::error::EngineError;

impl RustygradeEngine {
    /// Load, change and save the collections under the collections lock (no lost updates).
    pub(crate) fn update_collections(&self, change: impl FnOnce(&mut CollectionsData)) {
        self.update_collections_if(|d| {
            change(d);
            true
        });
    }

    /// Like [`Self::update_collections`], but the closure says whether it changed anything: when it
    /// did not, the file is left untouched.
    pub(crate) fn update_collections_if(&self, change: impl FnOnce(&mut CollectionsData) -> bool) {
        let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut data = CollectionsStore::load();
        if change(&mut data) {
            CollectionsStore::save(&data);
        }
    }

    /// Like [`Self::update_collections_if`], but a failed write is returned instead of only logged,
    /// and `after_save` runs under the collections lock once `native-collections.json` is on disk:
    /// callers that delete files depending on the saved state (playlist covers) must know whether
    /// the new state is on disk and inspect surviving references atomically. `Ok(true)` = changed and saved.
    pub(crate) fn update_collections_checked<T>(
        &self,
        change: impl FnOnce(&mut CollectionsData) -> Option<T>,
        after_save: impl FnOnce(&CollectionsData, T),
    ) -> Result<bool, EngineError> {
        let _guard = self.collections_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut data = CollectionsStore::load();
        let Some(token) = change(&mut data) else {
            return Ok(false);
        };
        CollectionsStore::try_save(&data).map_err(|e| {
            rustygrade_core::error_log::write("collections", &format!("could not save native-collections.json: {e}"));
            EngineError::Library(format!("could not save collections: {e}"))
        })?;
        after_save(&data, token);
        Ok(true)
    }

    pub(crate) fn update_settings_field(&self, change: impl FnOnce(&mut AppSettings)) {
        self.update_settings(change);
    }

    /// Load, change and save the settings under the settings lock (no lost updates).
    pub(super) fn update_settings(&self, change: impl FnOnce(&mut AppSettings)) {
        let _guard = self.settings_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = AppSettings::load();
        change(&mut s);
        s.save();
    }

    pub(super) fn update_saf_trees(&self, change: impl FnOnce(&mut Vec<String>)) {
        self.update_settings(|s| {
            let mut trees: Vec<String> = s
                .extra
                .get(KEY_SAF_TREES)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            change(&mut trees);
            s.extra.insert(
                KEY_SAF_TREES.to_owned(),
                Value::Array(trees.into_iter().map(Value::String).collect()),
            );
        });
    }

    pub(crate) fn invalidate_cache(&self) {
        self.invalidate();
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.cache.write().unwrap_or_else(|p| p.into_inner()) = None;
    }

    pub(crate) fn library(&self) -> Result<Arc<LibraryData>, EngineError> {
        if let Some(data) = self.cached() {
            return Ok(data);
        }
        // One loader at a time: callers that missed the cache together wait here and then find
        // the freshly loaded data instead of all parsing `library.json`.
        let _load = self.load_lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(data) = self.cached() {
            return Ok(data);
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let albums = LibraryStore::load()?;
        let artists = group_artists(&albums);
        let data = Arc::new(LibraryData::new(
            generation,
            albums,
            artists,
            LibraryStore::restored_from_backup(),
        ));
        let mut cache = self.cache.write().unwrap_or_else(|p| p.into_inner());
        // A scan/delete finished while we were reading the file: serve our data but do not keep it.
        if self.generation.load(Ordering::SeqCst) == generation {
            *cache = Some(Arc::clone(&data));
        }
        Ok(data)
    }

    fn cached(&self) -> Option<Arc<LibraryData>> {
        self.cache.read().unwrap_or_else(|p| p.into_inner()).as_ref().map(Arc::clone)
    }
}

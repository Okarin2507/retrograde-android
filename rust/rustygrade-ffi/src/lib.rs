//! UniFFI bridge between the Android (Kotlin) UI and `rustygrade-core`.
//!
//! The surface is coarse-grained on purpose: [`RustygradeEngine`] (library, scanning, collections,
//! stats, settings, backup, lyrics, tags) and `PlayerEngine` (queue, play counting, session) return
//! whole snapshots, so Compose never crosses the FFI once per row.

mod audio_fx;
mod backup_api;
mod collections_api;
mod dto;
mod engine;
mod error;
mod phase4_api;
mod player;
mod usb_audio;

pub use dto::*;
pub use audio_fx::*;
pub use backup_api::*;
pub use collections_api::*;
pub use engine::{RustygradeEngine, ScanProgressListener, UriOpener};
pub use player::*;
pub use error::EngineError;
pub use phase4_api::*;
pub use usb_audio::*;

uniffi::setup_scaffolding!();

/// Version of the Rust core, shown on the splash screen as `RUSTYGRADE // VER x.y.z`.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_cargo() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }
}

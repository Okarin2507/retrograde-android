use rustygrade_core::library::LibraryError;

/// Errors surfaced to Kotlin. Flat: the Kotlin side sees the message string only.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum EngineError {
    #[error("library error: {0}")]
    Library(String),
    #[error("a scan is already running")]
    ScanInProgress,
}

impl From<LibraryError> for EngineError {
    fn from(e: LibraryError) -> Self {
        EngineError::Library(e.to_string())
    }
}

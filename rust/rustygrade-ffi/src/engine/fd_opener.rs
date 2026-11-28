use std::fs::File;
use std::os::fd::FromRawFd;
use std::sync::Arc;

use rustygrade_core::FileOpener;

use super::UriOpener;

pub(crate) struct FdOpener(pub(super) Arc<dyn UriOpener>);

impl FdOpener {
    pub(crate) fn new(inner: Arc<dyn UriOpener>) -> Self {
        Self(inner)
    }
}

impl FileOpener for FdOpener {
    fn open(&self, uri: &str) -> Option<File> {
        let fd = self.0.open_fd(uri.to_owned())?;
        // 0..=2 are stdin/stdout/stderr: never a detached document fd, and closing one when the
        // `File` drops would break the process's own streams.
        if fd <= 2 {
            return None;
        }
        // SAFETY: the `UriOpener` contract hands over a freshly detached fd that nothing else owns.
        Some(unsafe { File::from_raw_fd(fd) })
    }
}

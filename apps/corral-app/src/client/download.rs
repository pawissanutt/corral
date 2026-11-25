//! An artifact's download: its bytes go page by page to a sink the platform supplies (a private
//! temporary file beside the chosen path, browser Blob parts), never through the detail cache.
use std::{fmt, future::Future, pin::Pin};

use nsh_core::prelude::Signal;

use crate::model::artifacts::download::Saving;

/// Puts the completed file in place; answers where, as the platform names it.
pub(crate) type SaveFinish = Pin<Box<dyn Future<Output = Result<String, String>>>>;

/// One page written; the next page is asked for only once it is, so a slow disk bounds what
/// waits in memory to one page.
pub(crate) type SaveStep<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + 'a>>;

/// Where a download's bytes go. Dropped without `finish`, it leaves no file behind.
pub(crate) trait SaveSink {
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> SaveStep<'a>;
    fn finish(self: Box<Self>) -> SaveFinish;
}

#[cfg(target_arch = "wasm32")]
pub(crate) type SaveOutput = Box<dyn SaveSink>;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type SaveOutput = Box<dyn SaveSink + Send>;

pub struct SaveArtifact {
    pub(crate) id: String,
    /// The whole artifact's reference, never a cited section's.
    pub(crate) reference: String,
    /// The catalog's size and SHA-256 (lowercase hex) the bytes must match.
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    pub(crate) status: Signal<Saving>,
    pub(crate) output: SaveOutput,
}

impl fmt::Debug for SaveArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveArtifact")
            .field("id", &self.id)
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}

//! Built-in system map facts from immutable Git objects. Does not use an extension,
//! invoke a model, or inspect the working tree for source files.
//!
//! Resolve a moving ref when it changes, then reuse its [`Revision`] to request facts.
//! Structure entries are shared by tree hash; activity is keyed separately by commit,
//! since different commits with the same tree can have different reachable histories.
//! A repeat request with the same resolved revision makes no Git calls.

mod cache;
mod delta;
mod entries;
mod git;
pub mod lenses;
mod manifests;
mod model;
pub mod provider;
mod runtime;
mod structure;
mod syntax;

pub use cache::FactsCache;
pub use entries::compare as compare_entry_points;
pub use model::*;
pub use structure::is_manifest;

use std::path::Path;

use anyhow::Result;

/// Resolve a Git revision (such as `HEAD` or a task commit SHA) to immutable IDs.
pub fn resolve_revision(repo: &Path, revision: &str) -> Result<Revision> {
    git::revision(repo, revision)
}

#[cfg(test)]
mod tests;

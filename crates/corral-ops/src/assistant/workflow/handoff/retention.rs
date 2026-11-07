use anyhow::{Result, ensure};
use corral_core::Store;
use std::path::Path;

/// Scratch disposal/GC cannot treat a handoff fence as ordinary inactivity. The source stays
/// retained until the lifecycle owner proves adoption and explicitly releases that reference.
/// Missing/corrupt metadata refuses cleanup rather than erasing the recovery source.
pub fn require_source_releasable(store: &Store, session: &str, path: &Path) -> Result<()> {
    let container = store.get_assistant_container(session)?;
    let Some(reference) = container
        .workflow
        .as_ref()
        .and_then(|w| w.current.handoff.as_ref())
    else {
        return Ok(());
    };
    let package: super::Package = serde_json::from_slice(
        &super::Content {
            blob: reference.blob.clone(),
            bytes: reference.bytes as usize,
        }
        .load(store)?,
    )?;
    ensure!(
        package.version == super::VERSION,
        "handoff_incompatible: source retention record requires upgrade"
    );
    ensure!(
        package.source_path.as_deref() != Some(path),
        "handoff_source_retained: {} remains the recovery source; adoption/release must be proven before cleanup",
        path.display()
    );
    Ok(())
}

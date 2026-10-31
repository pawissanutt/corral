//! Assistant scratch directories that no session record lists.
use anyhow::Result;
use corral_core::Store;

use super::Check;

pub(super) fn check(store: &Store) -> Result<Check> {
    let orphans = corral_ops::assistant::scratch::orphans(store)?;
    Ok(Check {
        id: "assistant_scratch",
        ok: orphans.is_empty(),
        hard: false,
        message: if orphans.is_empty() {
            "Every assistant scratch directory belongs to a session record.".into()
        } else {
            format!("Unrecorded assistant scratch directories: {}", orphans.join("; "))
        },
        fix: (!orphans.is_empty()).then(|| {
            "Inspect each directory and save any work before removing it with `git worktree remove`; corral never removes unrecorded scratch.".into()
        }),
    })
}

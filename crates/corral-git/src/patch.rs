use std::path::Path;

use anyhow::Result;

use crate::history::{git_utf8, validate_commit};

/// Committed patch bytes, ignoring blob IDs and hunk locations/function context.
/// Unlike whitespace-insensitive patch IDs, this retains every added/removed byte,
/// path, file mode and binary payload. No external diff driver or textconv runs.
pub fn review_patch(repo: &Path, base: &str, head: &str) -> Result<String> {
    validate_commit(repo, base)?;
    validate_commit(repo, head)?;
    let diff = git_utf8(
        repo,
        &[
            "-c",
            "diff.algorithm=myers",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--binary",
            "--full-index",
            "--unified=0",
            "--no-indent-heuristic",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            base,
            head,
            "--",
        ],
    )?;
    Ok(diff
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("index ") && !line.starts_with("@@ "))
        .collect())
}

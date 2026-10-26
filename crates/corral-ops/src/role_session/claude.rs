//! Claude uses an exact UUID within its project transcript directory.
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub(super) fn location(cwd: &Path, nonce: &Path) -> Option<(String, PathBuf)> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))?;
    let hash = format!("{:x}", Sha256::digest(nonce.as_os_str().as_encoded_bytes()));
    let id = format!(
        "{}-{}-4{}-a{}-{}",
        &hash[..8],
        &hash[8..12],
        &hash[13..16],
        &hash[17..20],
        &hash[20..32]
    );
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    Some((id, root.join("projects").join(project)))
}

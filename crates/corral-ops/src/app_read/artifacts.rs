//! Canonical catalog-only previews. Original source paths are never read or accepted.
use std::io::Read;

use anyhow::{Context, Result, ensure};
use corral_core::{Artifact, Store};
use corral_protocol::detail::{ArtifactEncoding, ArtifactPage, DetailBody};

use crate::Ctx;

const PAGE_BYTES: usize = 8 * 1024;
// Decoded discard work is bounded even for compressed legacy snapshots.
const MAX_OFFSET: u64 = 64 * 1024 * 1024;

pub(super) fn select(ctx: &Ctx, task: &str, key: &str) -> Result<Artifact> {
    let owner = if task.starts_with("A-") {
        corral_core::assistant::validate_id(task)?;
        task.to_owned()
    } else {
        Store::normalize_id(task)?
    };
    ensure!(
        !key.chars().any(char::is_control),
        "invalid artifact catalog key"
    );
    if key.starts_with("artifacts/") {
        ensure!(
            key.split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
            "invalid artifact catalog key"
        );
        ensure!(
            crate::artifact_storage::task_of(key) == Some(owner.as_str()),
            "artifact does not belong to selected owner"
        );
    } else {
        ensure!(
            key.starts_with(&format!("{owner}:")) && !key.contains('/'),
            "artifact preview requires a canonical catalog key"
        );
    }
    let artifact = crate::artifact_storage::resolve_artifact(ctx, key)?;
    ensure!(
        crate::artifact_storage::task_of(&artifact.path) == Some(owner.as_str()),
        "artifact does not belong to selected owner"
    );
    let reference = format!(
        "{owner}:{}:{}",
        artifact.run,
        corral_core::snapshot_name(&artifact)
    );
    ensure!(
        key == artifact.path || key == reference,
        "artifact key is not an exact catalog reference"
    );
    Ok(artifact)
}

pub(super) fn identity(artifact: &Artifact) -> Result<String> {
    super::digest(&(
        artifact.path.as_str(),
        artifact.sha256.as_str(),
        artifact.bytes,
        artifact.compressed_bytes,
        artifact.purged_at,
        artifact.added,
    ))
}

pub(super) fn page(
    ctx: &Ctx,
    task: &str,
    key: &str,
    artifact: &Artifact,
    offset: usize,
) -> Result<(DetailBody, Option<usize>)> {
    let offset_u64 = u64::try_from(offset).context("invalid artifact page offset")?;
    ensure!(
        offset_u64 <= MAX_OFFSET,
        "artifact page offset exceeds bounded decoded-read limit"
    );
    let mut reader = crate::artifact_storage::artifact_reader_confined(ctx, artifact)?;
    let skipped = std::io::copy(&mut reader.by_ref().take(offset_u64), &mut std::io::sink())
        .context("seeking decoded artifact page")?;
    ensure!(
        skipped == offset_u64,
        "artifact page offset exceeds source content"
    );
    let mut bytes = Vec::with_capacity(PAGE_BYTES + 1);
    reader
        .take((PAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("reading confined artifact page")?;
    let more = bytes.len() > PAGE_BYTES;
    bytes.truncate(PAGE_BYTES);
    let next = more
        .then(|| {
            offset
                .checked_add(bytes.len())
                .context("artifact page offset overflow")
        })
        .transpose()?;
    Ok((
        DetailBody::Artifact(ArtifactPage {
            task: task.to_owned(),
            key: key.to_owned(),
            media_type: None,
            encoding: ArtifactEncoding::Base64,
            content: base64(&bytes),
            truncated: more,
        }),
        next,
    ))
}

// An independently encoded chunk lets the client preserve arbitrary artifact bytes.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[usize::from(a >> 2)] as char);
        output.push(ALPHABET[usize::from((a & 3) << 4 | b >> 4)] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[usize::from((b & 15) << 2 | c >> 6)] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[usize::from(c & 63)] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn artifact_chunks_preserve_binary_base64_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0, 255, 128]), "AP+A");
    }
}

#[cfg(test)]
#[test]
fn catalog_selector_refuses_host_paths_and_other_owners() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let ctx = Ctx::from_store(Store::init(directory.path())?);
    for key in [
        "/etc/passwd",
        "1",
        "artifacts/T-002/R1/report",
        "artifacts/T-001/../report",
        "T-001:R1:../../report",
    ] {
        assert!(select(&ctx, "T-001", key).is_err(), "accepted {key}");
    }
    Ok(())
}

//! Q20: cold selected-body preparation, never ordinary head discovery. A zstd blob is not
//! seekable: expose the additional decoded input I/O, retain only the logical prefix/tail,
//! and consume through EOF so the canonical reader verifies its immutable hash.
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{
    Artifact, ArtifactKind,
    blob_ref::BlobHash,
    work::{self, Counter},
};
use std::collections::VecDeque;
use std::io::Read;

pub(super) fn canonical(
    ctx: &Ctx,
    artifact: &Artifact,
    hash: &BlobHash,
    declared: u64,
    cap: usize,
) -> Result<Option<String>> {
    ensure!(
        artifact.bytes == declared,
        "lesson artifact declared length mismatch: {}",
        artifact.path
    );
    if cap == 0
        || artifact
            .compressed_bytes
            .is_some_and(|bytes| bytes > cap as u64)
    {
        return Ok(None);
    }
    let Some(reader) = ctx.store.blob_reader(hash)? else {
        return Ok(None);
    };
    let recent = artifact.kind == ArtifactKind::Log && artifact.compressed_bytes.is_none();
    let selected = select(reader, declared, cap, recent)?;
    Ok(Some(String::from_utf8_lossy(&selected).into_owned()))
}

fn select(reader: impl Read, declared: u64, cap: usize, recent: bool) -> Result<Vec<u8>> {
    // The one-byte sentinel detects a lying length without unbounded input. Valid content
    // consumes exactly its declared bytes plus an EOF probe that completes verification.
    let mut reader = reader.take(declared.saturating_add(1));
    let mut read = 0u64;
    let mut window = VecDeque::new();
    let keep = cap.saturating_add(usize::from(recent));
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader
            .read(&mut chunk)
            .context("stream selected canonical lesson artifact")?;
        if n == 0 {
            break;
        }
        read = read
            .checked_add(n as u64)
            .context("selected artifact stream length overflow")?;
        work::counted(Counter::SelectedArtifactBytesRead, n as u64);
        ensure!(
            read <= declared,
            "lesson artifact exceeds declared raw length"
        );
        if recent {
            for byte in &chunk[..n] {
                if window.len() == keep {
                    window.pop_front();
                }
                window.push_back(*byte);
            }
        } else {
            let remaining = cap.saturating_sub(window.len());
            window.extend(chunk[..n.min(remaining)].iter().copied());
        }
    }
    ensure!(
        read == declared,
        "lesson artifact is shorter than declared raw length"
    );
    let mut bytes: Vec<u8> = window.into_iter().collect();
    if recent && read > cap as u64 {
        let previous = bytes.remove(0);
        if previous != b'\n' {
            let end = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |n| n + 1);
            bytes.drain(..end);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_ceiling_rejects_short_and_long_inputs_without_draining_the_rest() {
        assert!(select(&b"short"[..], 6, 3, true).is_err());
        struct Ceiling {
            read: usize,
        }
        impl Read for Ceiling {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                assert!(
                    self.read < 7,
                    "reader crossed declared length plus sentinel"
                );
                let n = out.len().min(7 - self.read);
                out[..n].fill(b'x');
                self.read += n;
                Ok(n)
            }
        }
        assert!(select(Ceiling { read: 0 }, 6, 3, true).is_err());
    }
}

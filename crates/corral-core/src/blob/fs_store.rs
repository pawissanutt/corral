//! The filesystem blob store (plan T-253 sections 1 and 7): zstd-compressed content at
//! `<root>/sha256/<2>/<62>`, staged in `<root>/staging/` on the same filesystem so publication
//! is one rename. Memory stays bounded for any input size: hashing and compression stream.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use sha2::{Digest, Sha256};

use crate::backend::{BlobFile, BlobStore, Staged, StagedData, VerifiedReader};
use crate::blob_ref::BlobHash;

/// The codec of every blob this store writes.
pub const CODEC: &str = "zstd";
const LEVEL: i32 = 3;
const CHUNK: usize = 128 * 1024;
const HASH_DIR: &str = "sha256";
const STAGING_DIR: &str = "staging";

pub struct FsBlobStore {
    root: PathBuf,
    hashes: PathBuf,
    staging: PathBuf,
}

impl FsBlobStore {
    /// The store under `<corral_dir>/blobs`, creating its directories.
    pub fn open(corral_dir: &Path) -> Result<Self> {
        Self::at(corral_dir.join("blobs"))
    }

    pub fn at(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let hashes = root.join(HASH_DIR);
        let staging = root.join(STAGING_DIR);
        for dir in [&hashes, &staging] {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        Ok(Self {
            root,
            hashes,
            staging,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging
    }

    /// Where the content of `hash` lives.
    pub fn path_of(&self, hash: &BlobHash) -> PathBuf {
        let (fanout, name) = hash.as_str().split_at(2);
        self.hashes.join(fanout).join(name)
    }

    /// Removes staged files last modified before `older_than`: what a process that died
    /// between `stage` and `put_staged` left behind. Returns how many it removed.
    pub fn sweep_staging(&self, older_than: Timestamp) -> Result<u64> {
        let mut removed = 0;
        let entries = fs::read_dir(&self.staging)
            .with_context(|| format!("list {}", self.staging.display()))?;
        for entry in entries {
            let path = entry
                .with_context(|| format!("list {}", self.staging.display()))?
                .path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("stat {}", path.display()));
                }
            };
            if !meta.is_file() || modified(&meta, &path)? >= older_than {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("remove {}", path.display()));
                }
            }
        }
        Ok(removed)
    }

    /// Streams `src` into a staging file: its sha256, raw size, zstd size and path.
    fn stage_file(&self, src: &mut dyn Read) -> Result<(BlobHash, u64, u64, PathBuf)> {
        let tmp = tempfile::Builder::new()
            .prefix("stage-")
            .tempfile_in(&self.staging)
            .with_context(|| format!("create a staging file in {}", self.staging.display()))?;
        let mut encoder = zstd::stream::write::Encoder::new(
            BufWriter::with_capacity(CHUNK, tmp.as_file()),
            LEVEL,
        )
        .context("start a zstd stream")?;
        let mut hasher = Sha256::new();
        let mut raw_bytes = 0u64;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("read blob content"),
            };
            hasher.update(&buf[..n]);
            encoder
                .write_all(&buf[..n])
                .context("compress blob content")?;
            raw_bytes += n as u64;
        }
        encoder
            .finish()
            .context("finish the zstd stream")?
            .into_inner()
            .map_err(io::IntoInnerError::into_error)
            .context("write the staging file")?;
        let stored_bytes = tmp
            .as_file()
            .metadata()
            .context("stat the staging file")?
            .len();
        let digest: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let hash = BlobHash::parse(&digest)?;
        let (_, path) = tmp.keep().context("keep the staging file")?;
        Ok((hash, raw_bytes, stored_bytes, path))
    }

    /// Moves a complete staging file to its content address, or drops it when the content is
    /// already there. The staging file never outlives this call.
    fn publish(&self, staged: &Path, hash: &BlobHash) -> Result<()> {
        let dest = self.path_of(hash);
        let published = self.publish_at(staged, &dest);
        if published.is_err() {
            // The primary error is what the caller needs; a staging file this removal misses
            // is left for `sweep_staging`.
            let _ = fs::remove_file(staged);
        }
        published.with_context(|| format!("publish blob {hash} at {}", dest.display()))
    }

    fn publish_at(&self, staged: &Path, dest: &Path) -> Result<()> {
        if fs::symlink_metadata(dest).is_ok() {
            return fs::remove_file(staged).context("remove the duplicate staging file");
        }
        OpenOptions::new()
            .write(true)
            .open(staged)
            .and_then(|file| file.sync_all())
            .context("sync the staging file")?;
        let fanout = dest.parent().context("blob path has a parent")?;
        if !fanout.is_dir() {
            fs::create_dir_all(fanout).context("create the fan-out directory")?;
            sync_dir(&self.hashes)?;
        }
        fs::rename(staged, dest).context("rename the staging file")?;
        sync_dir(fanout)
    }
}

fn modified(meta: &fs::Metadata, path: &Path) -> Result<Timestamp> {
    let at = meta
        .modified()
        .with_context(|| format!("read the mtime of {}", path.display()))?;
    Timestamp::try_from(at).with_context(|| format!("mtime of {}", path.display()))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("sync directory {}", dir.display()))
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<()> {
    Ok(())
}

fn hex_names(dir: &Path, len: usize) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("list {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("list {}", dir.display()))?;
        if let Some(name) = entry.file_name().to_str()
            && name.len() == len
            && name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    Ok(names)
}

/// Decoder errors mean the file no longer holds what was stored under its hash.
struct Decoded<R> {
    inner: R,
    hash: BlobHash,
}

impl<R: Read> Read for Decoded<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).map_err(|error| {
            if error.kind() == io::ErrorKind::Interrupted {
                return error;
            }
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("HashMismatch: blob {} does not decode: {error}", self.hash),
            )
        })
    }
}

impl BlobStore for FsBlobStore {
    fn stage(&self, src: &mut dyn Read) -> Result<Staged> {
        let (hash, raw_bytes, stored_bytes, path) = self.stage_file(src)?;
        Ok(Staged {
            hash,
            raw_bytes,
            stored_bytes,
            codec: CODEC.to_owned(),
            data: StagedData::File(path),
        })
    }

    fn put_staged(&self, staged: Staged) -> Result<()> {
        let hash = staged.hash;
        match staged.data {
            StagedData::File(path) => {
                ensure!(
                    staged.codec == CODEC,
                    "staged blob {hash} holds {} content; this store keeps {CODEC}",
                    staged.codec
                );
                self.publish(&path, &hash)
            }
            StagedData::Memory(bytes) => {
                ensure!(
                    BlobHash::of(&bytes) == hash,
                    "staged content does not match {hash}"
                );
                let (_, _, _, path) = self.stage_file(&mut bytes.as_slice())?;
                self.publish(&path, &hash)
            }
        }
    }

    fn get(&self, hash: &BlobHash) -> Result<VerifiedReader> {
        let path = self.path_of(hash);
        let file =
            File::open(&path).with_context(|| format!("open blob {hash} at {}", path.display()))?;
        let decoder = zstd::stream::read::Decoder::new(file)
            .with_context(|| format!("start decoding blob {hash}"))?;
        let decoded = Decoded {
            inner: decoder,
            hash: hash.clone(),
        };
        Ok(VerifiedReader::new(Box::new(decoded), hash.clone()))
    }

    fn delete(&self, hash: &BlobHash) -> Result<()> {
        let path = self.path_of(hash);
        match fs::remove_file(&path) {
            Ok(()) => sync_dir(path.parent().context("blob path has a parent")?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("unlink blob {hash}")),
        }
    }

    fn list(&self, after: Option<&BlobHash>, limit: u32) -> Result<Vec<BlobFile>> {
        let limit = usize::try_from(limit)?;
        let mut files = Vec::new();
        if limit == 0 {
            return Ok(files);
        }
        let floor = after.map(|hash| &hash.as_str()[..2]);
        for fanout in hex_names(&self.hashes, 2)? {
            if floor.is_some_and(|floor| fanout.as_str() < floor) {
                continue;
            }
            let dir = self.hashes.join(&fanout);
            for name in hex_names(&dir, 62)? {
                let hash = BlobHash::parse(&format!("{fanout}{name}"))?;
                if after.is_some_and(|after| &hash <= after) {
                    continue;
                }
                let path = dir.join(&name);
                let meta = match fs::symlink_metadata(&path) {
                    Ok(meta) => meta,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).with_context(|| format!("stat {}", path.display()));
                    }
                };
                if !meta.is_file() {
                    continue;
                }
                files.push(BlobFile {
                    hash,
                    stored_bytes: meta.len(),
                    modified: modified(&meta, &path)?,
                });
                if files.len() == limit {
                    return Ok(files);
                }
            }
        }
        Ok(files)
    }
}

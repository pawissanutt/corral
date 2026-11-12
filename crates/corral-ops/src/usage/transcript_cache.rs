//! Cache decoded lines, not run-specific windows, model defaults or prices.

use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, anyhow};

use super::{SessionLine, parse_session_line};

static TRANSCRIPTS: OnceLock<Mutex<Cache>> = OnceLock::new();
const ANCHOR_LEN: u64 = 64;

#[derive(Default)]
struct Cache {
    scopes: HashMap<PathBuf, ScopedCache>,
}

#[derive(Default)]
struct ScopedCache {
    generation: u64,
    transcripts: HashMap<PathBuf, Transcript>,
}

#[derive(Default)]
struct Transcript {
    generation: u64,
    identity: String,
    parsed_offset: u64,
    observed_len: u64,
    anchor: Vec<u8>,
    lines: Vec<SessionLine>,
    partial: Vec<u8>,
    #[cfg(test)]
    reads: usize,
    #[cfg(test)]
    decoded: usize,
}

fn identity(metadata: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", metadata.created())
    }
}

fn trailing(file: &mut File, position: u64) -> Result<Vec<u8>> {
    let len = position.min(ANCHOR_LEN);
    file.seek(SeekFrom::Start(position - len))
        .context("seeking transcript anchor")?;
    let mut bytes = vec![0; usize::try_from(len)?];
    file.read_exact(&mut bytes)
        .context("reading transcript anchor")?;
    Ok(bytes)
}

impl Transcript {
    fn refresh(&mut self, mut file: File) -> Result<()> {
        let metadata = file.metadata().context("inspecting transcript")?;
        let identity = identity(&metadata);
        let reset = self.identity != identity
            || metadata.len() < self.observed_len
            || trailing(&mut file, self.observed_len)? != self.anchor;
        if reset {
            self.identity = identity;
            self.parsed_offset = 0;
            self.observed_len = 0;
            self.anchor.clear();
            self.lines.clear();
            self.partial.clear();
        }
        if metadata.len() == self.observed_len {
            return Ok(());
        }
        // The last unterminated line is provisional, including valid JSON at EOF.
        // It is replaced on append, never retained as a second usage response.
        if !self.partial.is_empty() {
            self.lines.pop();
        }
        file.seek(SeekFrom::Start(self.observed_len))
            .context("seeking transcript append")?;
        #[cfg(test)]
        {
            self.reads += 1;
        }
        {
            let mut reader = BufReader::new((&mut file).take(metadata.len() - self.observed_len));
            loop {
                let n = reader
                    .read_until(b'\n', &mut self.partial)
                    .context("reading session line")?;
                if n == 0 {
                    break;
                }
                corral_core::work::counted(
                    corral_core::work::Counter::TranscriptBytesRead,
                    u64::try_from(n)?,
                );
                self.observed_len = self
                    .observed_len
                    .checked_add(u64::try_from(n)?)
                    .context("transcript offset overflow")?;
                if self.partial.last() == Some(&b'\n') {
                    self.push_line();
                    self.partial.clear();
                    self.parsed_offset = self.observed_len;
                }
            }
        }
        if self.observed_len > self.parsed_offset {
            self.push_line();
        }
        self.anchor = trailing(&mut file, self.observed_len)?;
        Ok(())
    }

    fn push_line(&mut self) {
        self.lines
            .push(parse_session_line(&self.partial, self.lines.len()));
        #[cfg(test)]
        {
            self.decoded += 1;
        }
    }
}

fn cache() -> Result<MutexGuard<'static, Cache>> {
    TRANSCRIPTS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| anyhow!("transcript cache lock poisoned"))
}

/// Store roots namespace both transcript identities and refresh generations. An unrelated
/// store's board pass must not evict another store's warmed transcripts.
fn scoped<'a>(cache: &'a mut Cache, root: &Path) -> Result<&'a mut ScopedCache> {
    if !cache.scopes.contains_key(root) {
        cache
            .scopes
            .insert(root.to_path_buf(), ScopedCache::default());
    }
    cache
        .scopes
        .get_mut(root)
        .context("missing transcript cache scope")
}

/// A board refresh retains only this store's transcripts used during its generation.
pub(super) fn refresh(root: &Path, collect: impl FnOnce()) -> Result<()> {
    let generation = {
        let mut cache = cache()?;
        let scope = scoped(&mut cache, root)?;
        scope.generation = scope.generation.wrapping_add(1);
        scope.generation
    };
    collect();
    let mut cache = cache()?;
    let scope = scoped(&mut cache, root)?;
    // A newer concurrent refresh of this store owns pruning, as before.
    if scope.generation == generation {
        scope
            .transcripts
            .retain(|_, transcript| transcript.generation == generation);
    }
    Ok(())
}

pub(super) fn with_lines<T>(
    root: &Path,
    path: &Path,
    aggregate: impl FnOnce(&[SessionLine]) -> Result<T>,
) -> Result<T> {
    let mut cache = cache()?;
    let scope = scoped(&mut cache, root)?;
    let transcript = scope.transcripts.entry(path.to_path_buf()).or_default();
    transcript.generation = scope.generation;
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if let Err(error) = transcript
        .refresh(file)
        .with_context(|| format!("parsing {}", path.display()))
    {
        // Do not reuse a partially updated checkpoint after a failed read.
        scope.transcripts.remove(path);
        return Err(error);
    }
    corral_core::work::counted(
        corral_core::work::Counter::TranscriptLinesAggregated,
        u64::try_from(transcript.lines.len())?,
    );
    aggregate(&transcript.lines).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
pub(super) fn counts(root: &Path, path: &Path) -> (usize, usize, u64) {
    let cache = cache().unwrap();
    let transcript = &cache.scopes[root].transcripts[path];
    (
        transcript.reads,
        transcript.decoded,
        transcript.parsed_offset,
    )
}

/// Forgets only the named store's cache, without invalidating concurrent fixtures.
#[cfg(test)]
pub(super) fn clear(root: &Path) {
    cache().unwrap().scopes.remove(root);
}

#[cfg(test)]
pub(super) fn contains(root: &Path, path: &Path) -> bool {
    cache()
        .unwrap()
        .scopes
        .get(root)
        .is_some_and(|scope| scope.transcripts.contains_key(path))
}

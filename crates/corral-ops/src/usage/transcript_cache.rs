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

/// A board refresh retains only transcripts used during its generation.
pub(super) fn refresh(collect: impl FnOnce()) -> Result<()> {
    let generation = {
        let mut cache = cache()?;
        cache.generation = cache.generation.wrapping_add(1);
        cache.generation
    };
    collect();
    let mut cache = cache()?;
    // A newer concurrent refresh owns pruning; an older finisher must not
    // evict transcripts that were just used by that newer generation.
    if cache.generation == generation {
        cache
            .transcripts
            .retain(|_, transcript| transcript.generation == generation);
    }
    Ok(())
}

pub(super) fn with_lines<T>(
    path: &Path,
    aggregate: impl FnOnce(&[SessionLine]) -> Result<T>,
) -> Result<T> {
    let mut cache = cache()?;
    let generation = cache.generation;
    let transcript = cache.transcripts.entry(path.to_path_buf()).or_default();
    transcript.generation = generation;
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if let Err(error) = transcript
        .refresh(file)
        .with_context(|| format!("parsing {}", path.display()))
    {
        // Do not reuse a partially updated checkpoint after a failed read.
        cache.transcripts.remove(path);
        return Err(error);
    }
    aggregate(&transcript.lines).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
pub(super) fn counts(path: &Path) -> (usize, usize, u64) {
    let cache = cache().unwrap();
    let transcript = &cache.transcripts[path];
    (
        transcript.reads,
        transcript.decoded,
        transcript.parsed_offset,
    )
}

#[cfg(test)]
pub(super) fn contains(path: &Path) -> bool {
    cache().unwrap().transcripts.contains_key(path)
}

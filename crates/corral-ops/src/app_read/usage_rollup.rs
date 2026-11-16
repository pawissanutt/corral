//! Disposable full-history day buckets. Cache failures are misses, never missing accounting.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::AgentRunState;
use jiff::Timestamp;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use super::{Aggregate, aggregate::Saved};
use crate::{
    Ctx,
    agent::usage::read::{self, Inputs},
};

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Document<T> {
    version: u32,
    fingerprint: String,
    checksum: String,
    value: T,
}

pub(super) fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).context("encoding usage cache identity")?)
    ))
}

fn path(ctx: &Ctx, key: &str) -> Result<PathBuf> {
    Ok(ctx
        .store
        .cache_root()?
        .join("usage-v1")
        .join(format!("{}.json", fingerprint(&key)?)))
}

pub(super) fn load<T: DeserializeOwned + Serialize>(
    ctx: &Ctx,
    key: &str,
    identity: &str,
) -> Option<T> {
    let bytes = fs::read(path(ctx, key).ok()?).ok()?;
    let doc: Document<T> = serde_json::from_slice(&bytes).ok()?;
    (doc.version == VERSION
        && doc.fingerprint == identity
        && doc.checksum == fingerprint(&doc.value).ok()?)
    .then_some(doc.value)
}

/// An unavailable cache never prevents a complete source read. Atomic replacement keeps
/// concurrent daemon/CLI readers from seeing a half-written summary.
pub(super) fn save<T: Serialize>(ctx: &Ctx, key: &str, identity: &str, value: &T) {
    let save = || -> Result<()> {
        let path = path(ctx, key)?;
        let parent = path.parent().context("usage cache has no parent")?;
        fs::create_dir_all(parent).context("creating usage cache directory")?;
        let mut file =
            tempfile::NamedTempFile::new_in(parent).context("creating usage cache file")?;
        serde_json::to_writer(
            file.as_file_mut(),
            &Document {
                version: VERSION,
                fingerprint: identity.to_owned(),
                checksum: fingerprint(value)?,
                value,
            },
        )
        .context("writing usage cache")?;
        file.persist(&path).context("publishing usage cache")?;
        Ok(())
    };
    // This is derived data, never the only copy of usage evidence.
    let _ = save();
}

#[derive(Serialize)]
struct Stamp {
    len: u64,
    modified: (bool, u64, u32),
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

fn stamps(inputs: &Inputs) -> Result<Vec<(&Path, Option<Stamp>)>> {
    inputs
        .files
        .keys()
        .map(|path| {
            let meta = match fs::symlink_metadata(path) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok((path.as_path(), None));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("reading usage source {}", path.display()));
                }
            };
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            let modified = meta
                .modified()
                .with_context(|| format!("reading usage source timestamp {}", path.display()))?;
            let (positive, duration) = match modified.duration_since(std::time::UNIX_EPOCH) {
                Ok(duration) => (true, duration),
                Err(error) => (false, error.duration()),
            };
            Ok((
                path.as_path(),
                Some(Stamp {
                    len: meta.len(),
                    modified: (positive, duration.as_secs(), duration.subsec_nanos()),
                    #[cfg(unix)]
                    inode: meta.ino(),
                    #[cfg(unix)]
                    device: meta.dev(),
                    #[cfg(unix)]
                    changed: (meta.ctime(), meta.ctime_nsec()),
                }),
            ))
        })
        .collect()
}

fn identity(ctx: &Ctx, inputs: &Inputs, profile: &Option<String>) -> Result<String> {
    fingerprint(&(inputs, stamps(inputs)?, profile, pricing(ctx)?))
}

pub(super) fn pricing(ctx: &Ctx) -> Result<String> {
    fingerprint(&(
        &ctx.store.config.pricing,
        corral_core::pricing::CATALOG_SOURCE,
        corral_core::pricing::CATALOG_BEDROCK_SOURCE,
    ))
}

fn add(
    ctx: &Ctx,
    facts: &mut Aggregate,
    profile: &Option<String>,
    mut entry: corral_core::UsageEntry,
    now: Timestamp,
) {
    entry.reprice(&ctx.store.config);
    let connection = entry
        .connection
        .map(|c| c.id().to_owned())
        .unwrap_or_default();
    facts.add(connection, profile.clone(), entry, Timestamp::MIN, now);
}

#[path = "usage_closed.rs"]
mod closed;

pub(super) struct Source<'a> {
    pub key: &'a str,
    pub state: &'a AgentRunState,
    pub ended: bool,
    pub profile: Option<String>,
}

/// Closed files are folded completely once; only live files have a row/byte budget.
pub(super) fn state(
    ctx: &Ctx,
    source: Source<'_>,
    cutoff: Timestamp,
    now: Timestamp,
    max: usize,
) -> Result<Aggregate> {
    let Source {
        key,
        state,
        ended,
        profile,
    } = source;
    let mut inputs = read::inputs(state, None, &ctx.store)?;
    if inputs.files.is_empty() {
        anyhow::bail!("no bound usage source");
    }
    if ended {
        for input in inputs.files.values_mut() {
            input.closed = true;
        }
    }
    // Fence times do not bound delayed in-flight billing; only ended session records
    // can skip a whole source. Retired PM generations have no durable end timestamp.
    let whole_identity = identity(ctx, &inputs, &profile)?;
    let whole_key = format!("source:{key}");
    if let Some(saved) =
        load::<Saved>(ctx, &whole_key, &whole_identity).filter(|saved| saved.valid(now))
    {
        return Ok(saved.restore(cutoff));
    }
    let mut live = Inputs {
        files: BTreeMap::new(),
        bindings: inputs.bindings.clone(),
    };
    let mut closed = Inputs {
        files: BTreeMap::new(),
        bindings: inputs.bindings,
    };
    for (path, input) in inputs.files {
        if input.closed {
            closed.files.insert(path, input);
        } else {
            live.files.insert(path, input);
        }
    }
    let (live_entries, coverage) = read::collect(&live, Some(max))?;
    let mut all = closed::fold(ctx, key, &closed, &live, &live_entries, &profile, now)?;
    all.facts.budget.coverage.scanned_rows += coverage.scanned_rows as u64;
    if coverage.partial {
        all.facts
            .budget
            .partial(coverage.reason.unwrap_or("partial_transcript"));
    }
    for entry in live_entries {
        if all.precedes(&entry) {
            continue;
        }
        add(ctx, &mut all.facts, &profile, entry.usage, now);
    }
    let mut after = read::inputs(state, None, &ctx.store)?;
    if ended {
        for input in after.files.values_mut() {
            input.closed = true;
        }
    }
    if !all.facts.budget.coverage.partial
        && !all.facts.future
        && whole_identity == identity(ctx, &after, &profile)?
    {
        save(ctx, &whole_key, &whole_identity, &Saved::new(&all.facts));
    }
    Ok(all.facts.window(cutoff))
}

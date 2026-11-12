use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{CACHE_FILE, Projection};
use crate::Ctx;

const VERSION: u32 = 1;

#[derive(Serialize)]
struct CacheRef<'a> {
    version: u32,
    projection: &'a Projection,
}

#[derive(Deserialize)]
struct CacheFile {
    version: u32,
    projection: Projection,
}

/// The daemon's rebuildable cache location; this does not create the directory.
pub fn cache_path(ctx: &Ctx) -> Result<PathBuf> {
    Ok(ctx
        .store
        .cache_root()
        .context("locating board cache root")?
        .join(CACHE_FILE))
}

impl Projection {
    /// Atomically replace `boardview-v1.bin`. The versioned payload uses the workspace's
    /// existing serde_json codec; the filename does not promise a particular binary codec.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating board cache directory {}", parent.display()))?;
        let mut file = tempfile::NamedTempFile::new_in(parent).context("staging board cache")?;
        serde_json::to_writer(
            file.as_file_mut(),
            &CacheRef {
                version: VERSION,
                projection: self,
            },
        )
        .context("encoding board cache")?;
        file.as_file().sync_all().context("syncing board cache")?;
        file.persist(path)
            .with_context(|| format!("publishing board cache {}", path.display()))?;
        Ok(())
    }

    /// None means rebuild: absent/unreadable file, schema mismatch, decode failure or
    /// inconsistent cache. Cache bytes are never canonical and cannot fail store startup.
    pub fn load(path: impl AsRef<Path>) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
        if cache.version != VERSION {
            return None;
        }
        let mut projection = cache.projection;
        if projection.rows.iter().any(|(id, row)| {
            id != &row.id
                || row.rev == 0
                || row.rev > projection.folded_through.0
                || row.attention.len() > 4
                || row
                    .goal
                    .as_ref()
                    .is_some_and(|goal| goal.chars().count() > 80)
                || row.now.text.chars().count() > 60
                || row.cost_usd.is_some_and(|cost| !cost.is_finite())
                || projection.left.contains_key(id)
                || !projection.inputs.contains_key(id)
        }) || projection
            .left
            .values()
            .any(|rev| *rev == 0 || *rev > projection.folded_through.0)
            || projection
                .inputs
                .keys()
                .any(|id| !projection.rows.contains_key(id))
        {
            return None;
        }
        for (id, row) in &projection.rows {
            projection
                .revisions
                .entry(row.rev)
                .or_default()
                .insert(id.clone());
        }
        for (id, rev) in &projection.left {
            projection
                .revisions
                .entry(*rev)
                .or_default()
                .insert(id.clone());
        }
        for (id, inputs) in &projection.inputs {
            for input in inputs {
                projection
                    .dependents
                    .entry(input.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }
        Some(projection)
    }
}

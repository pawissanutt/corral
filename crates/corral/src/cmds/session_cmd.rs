use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_ops::Ctx;

use super::start_dir;

pub(super) fn hook(root: &Option<PathBuf>) {
    let result = start_dir(root)
        .and_then(|start| Ctx::open_for_sync(&start))
        .and_then(|ctx| corral_ops::sync_hook(&ctx).map(|_| ()));
    if let Err(e) = result {
        eprintln!("corral hook: {e:#}");
    }
}

pub(super) fn bind(root: &Option<PathBuf>, socket: &Path, launch_record: &Path) -> Result<()> {
    let ctx = Ctx::open_for_sync(&start_dir(root)?)?;
    let evidence = std::fs::read_to_string(launch_record)
        .with_context(|| format!("reading {}", launch_record.display()))?;
    ctx.store.bind_herdr_session(socket, &evidence)
}

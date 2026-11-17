//! Task-independent build-session gates, owner landing and durable records.
pub mod gate;
pub mod land;
pub mod paid;
pub mod record;
pub mod stats;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::Ctx;

/// Accept only host session ids, never paths or task ids.
pub fn validate_session_id(id: &str) -> Result<()> {
    let bytes = id.as_bytes();
    ensure!(
        (3..=83).contains(&bytes.len())
            && matches!(bytes[0], b'A' | b'B')
            && bytes[1] == b'-'
            && (bytes[2].is_ascii_lowercase() || bytes[2].is_ascii_digit())
            && bytes[3..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-'),
        "invalid build session id {id:?}"
    );
    Ok(())
}

/// Resolve the same data root passed to the corral-pi host, without creating it.
pub fn session_dir(id: &str) -> Result<PathBuf> {
    validate_session_id(id)?;
    Ok(crate::agent::install::data_root()?.join("build").join(id))
}

/// Owner-only build publication and landing cannot be authorized by agent attribution.
pub fn require_owner(ctx: &Ctx) -> Result<()> {
    let agent = ["AI_AGENT", "PI_CODING_AGENT", "CORRAL_ASSISTANT"]
        .iter()
        .any(|key| std::env::var_os(key).is_some());
    let pm = std::env::var_os("CORRAL_PM").is_some();
    let cwd = std::env::current_dir().context("reading build command directory")?;
    crate::automation_controls::require_owner_entry(&ctx.store, &cwd, agent, pm)
}

pub(super) fn project(ctx: &Ctx, key: &str, worktree: &Path) -> Result<(PathBuf, String)> {
    ensure!(worktree.is_absolute(), "worktree must be an absolute path");
    let repo = ctx.store.project_path(key)?;
    let config = ctx
        .store
        .config
        .projects
        .get(key)
        .context("project disappeared")?;
    let base = match &config.base {
        Some(base) => base.clone(),
        None => corral_git::current_branch(&repo).context("reading project base branch")?,
    };
    ensure!(
        common_dir(&repo)? == common_dir(worktree)?,
        "worktree does not belong to project {key:?}"
    );
    Ok((repo, base))
}

fn common_dir(path: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .stdin(std::process::Stdio::null())
        .output()
        .context("reading build worktree repository")?;
    ensure!(
        output.status.success(),
        "reading build worktree repository: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = String::from_utf8(output.stdout).context("decoding build worktree repository")?;
    Path::new(text.trim())
        .canonicalize()
        .context("resolving build worktree repository")
}

pub(super) fn private_dir(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .with_context(|| format!("creating {}", path.display()))?;
    ensure!(
        std::fs::symlink_metadata(path)
            .context("reading build directory metadata")?
            .file_type()
            .is_dir(),
        "build directory must not be a symlink: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

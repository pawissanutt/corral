//! Rebuild the compiled host for binaries built from a still-present checkout.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::bridge::assets::{asset_digest, find_bun, read_compat};
use super::install::{InstallRecord, install_compiled, parse_host_version, run_host};

pub(super) fn rebuild(
    source: &Path,
    sibling: &Path,
    root: &Path,
    built: Option<&str>,
) -> Result<InstallRecord> {
    let checkout = source
        .parent()
        .and_then(Path::parent)
        .context("invalid package path")?;
    let built = built
        .context("source build commit unavailable; run `cargo install` again or pass `--from`")?;
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(checkout)
        .stdin(Stdio::null())
        .output()
        .context("reading checkout HEAD with git")?;
    if !output.status.success() {
        bail!("cannot determine checkout HEAD; run `cargo install` again or pass `--from`");
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if head != built {
        bail!(
            "checkout HEAD {head} differs from binary build commit {built}; run `cargo install` again or pass `--from`"
        );
    }
    find_bun().context(
        "bun_unavailable: install bun on PATH (version pinned by packages/corral-pi/compat.json)",
    )?;
    let parent = sibling.parent().context("host has no parent directory")?;
    let staging = tempfile::Builder::new()
        .prefix(".corral-pi-host-")
        .tempdir_in(parent)
        .with_context(|| format!("staging host in {}", parent.display()))?;
    let host = staging.path().join("corral-pi-host");
    let output = Command::new(checkout.join("scripts/build-pi-host.sh"))
        .arg(&host)
        .current_dir(checkout)
        .stdin(Stdio::null())
        .output()
        .context("running scripts/build-pi-host.sh")?;
    if !output.status.success() {
        bail!(
            "building Pi host failed: {}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    // Validate the executable and its emitted assets without publishing an installation.
    let version = run_host(&host, &["--version".as_ref()])?;
    parse_host_version(&version).context("built host has an invalid version")?;
    let assets = staging.path().join("assets-check");
    std::fs::create_dir(&assets).context("creating host validation directory")?;
    run_host(&host, &["--write-install".as_ref(), assets.as_os_str()])?;
    read_compat(&assets)?;
    asset_digest(&assets)?;
    std::fs::rename(&host, sibling).with_context(|| format!("replacing {}", sibling.display()))?;
    let record = install_compiled(sibling, root)?;
    eprintln!(
        "Built Pi host commit {built} at {} (assets sha256 {})",
        sibling.display(),
        record.digest
    );
    Ok(record)
}

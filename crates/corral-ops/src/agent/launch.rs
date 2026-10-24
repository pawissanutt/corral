//! `corral agent launch --bootstrap <path> [--assets <dir>]`: the Rust launcher run in the
//! pane. It consumes the one-shot bootstrap, checks the asset digest, Bun range, GO manifest and
//! the role, delivery and auth modules, generates `host_cap` and `ctl_cap`, writes `ctl.cap` and
//! execs Bun by argv. Owner: T-252 child host-bridge.
//!
//! The runtime-directory layout lives here so run-end cleanup (integration-seams) and the
//! launcher agree on it: `$XDG_RUNTIME_DIR/corral/<store>/<recipient>/`, else an owner-only
//! `/tmp/corral-<uid>/<store>/<recipient>/`, with one `g<n>/` per generation below it.
use std::path::{Path, PathBuf};

use anyhow::Result;
use corral_core::Store;
use corral_core::agent::paths;
use corral_core::inbox::Recipient;

use super::cli::{AgentCommand, LaunchArgs, unavailable};

/// The T-252 child that lands the launcher and the launch command.
pub const OWNER: &str = "host-bridge";

pub fn launch(_start: &Path, _args: &LaunchArgs) -> Result<()> {
    Err(unavailable(AgentCommand::Launch))
}

/// The pane command for a pi-native container (`{corral} agent launch {delivery_args}` with a
/// freshly written bootstrap). Unavailable until host-bridge lands.
pub fn command(recipient: &Recipient) -> Result<String> {
    anyhow::bail!(
        "op_unavailable: the pi-native launch command for {} lands with T-252 child {OWNER}",
        recipient.key()?
    )
}

/// Root of every corral runtime directory for this user.
pub fn runtime_root() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("corral"),
        None => std::env::temp_dir().join(format!("corral-{}", uid())),
    }
}

/// The runtime directory of one container: every generation's socket, `ctl.cap` and bootstrap.
pub fn runtime_dir(store: &Store, recipient: &Recipient) -> Result<PathBuf> {
    runtime_dir_in(&runtime_root(), store, recipient)
}

/// `runtime_dir` under an explicit root.
pub fn runtime_dir_in(root: &Path, store: &Store, recipient: &Recipient) -> Result<PathBuf> {
    let key = recipient.key()?;
    Ok(root
        .join(paths::store_id(store))
        .join(&paths::sha256_hex(key.as_bytes())[..16]))
}

#[cfg(unix)]
fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

#[cfg(not(unix))]
fn uid() -> u32 {
    0
}

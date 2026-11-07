//! Generation secrets and their private control-capability file.
use super::{CTL_CAP_FILE, ensure_private_dir, write_private};
use anyhow::{Context, Result};
use std::path::Path;

/// The two directional secrets of one generation. Never logged; only digests persist.
pub struct Secrets {
    pub host_cap: String,
    pub ctl_cap: String,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets(<redacted>)")
    }
}

impl Secrets {
    pub fn generate() -> Result<Secrets> {
        let secret = || -> Result<String> {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("generating a secret: {e}"))?;
            Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
        };
        Ok(Secrets {
            host_cap: secret()?,
            ctl_cap: secret()?,
        })
    }
}

/// Writes `ctl.cap` 0600 by atomic rename, so any later corral process can reach the host.
#[cfg(unix)]
pub fn write_ctl_cap(dir: &Path, ctl_cap: &str) -> Result<()> {
    let parent = dir
        .parent()
        .context("the generation directory has no parent")?;
    ensure_private_dir(parent, dir)?;
    let staged = dir.join(format!("{CTL_CAP_FILE}.{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    write_private(&staged, ctl_cap.as_bytes())?;
    std::fs::rename(&staged, dir.join(CTL_CAP_FILE))
        .with_context(|| format!("installing {}", dir.join(CTL_CAP_FILE).display()))
}

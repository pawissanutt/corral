use std::{env, path::PathBuf, sync::OnceLock};

use anyhow::{Context, Result};

static BINARY: OnceLock<PathBuf> = OnceLock::new();

/// Capture the launch pathname before the command can wait in a long-running
/// operation (notably a detached pipeline gate). On Linux, asking for the
/// executable after an atomic reinstall returns the old inode's deleted name.
/// Do not canonicalize a configured pathname: its symlink may be the upgrade point.
pub fn pin_binary_path() -> Result<()> {
    if BINARY.get().is_none() {
        let _ = BINARY.set(resolve_binary()?);
    }
    Ok(())
}

pub(crate) fn binary_path() -> Result<PathBuf> {
    match BINARY.get() {
        Some(path) => Ok(path.clone()),
        None => resolve_binary(),
    }
}

fn resolve_binary() -> Result<PathBuf> {
    if let Some(bin) = env::var_os("CORRAL_NEXT_BIN").filter(|name| !name.is_empty()) {
        return std::path::absolute(bin).context("resolve CORRAL_NEXT_BIN");
    }
    let path = env::current_exe().context("resolve current corral executable")?;
    #[cfg(target_os = "linux")]
    if path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") && !path.exists() {
        anyhow::bail!(
            "the running Corral binary was replaced before startup completed; restart it or set CORRAL_NEXT_BIN to the installed Corral CLI"
        );
    }
    Ok(path)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::unix::fs::PermissionsExt, process::Command};

    #[test]
    fn running_launcher_keeps_install_path_across_binary_replacement() {
        const CHILD: &str = "CORRAL_TEST_REPLACED_BINARY";
        const TEST: &str =
            "binary::tests::running_launcher_keeps_install_path_across_binary_replacement";
        if env::var_os(CHILD).is_some() {
            let install = env::current_exe().unwrap();
            pin_binary_path().unwrap();
            // Replace the running launcher's inode as cargo install does. No timing dependency.
            let mut replacement =
                tempfile::NamedTempFile::new_in(install.parent().unwrap()).unwrap();
            replacement
                .write_all(b"#!/bin/sh\nprintf 'replacement\\n'\n")
                .unwrap();
            replacement
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o700))
                .unwrap();
            let replacement = replacement.into_temp_path();
            replacement.persist(&install).unwrap();
            assert!(
                env::current_exe()
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .ends_with(b" (deleted)")
            );
            let bin = binary_path().unwrap();
            assert_eq!(bin, install);
            assert_eq!(Command::new(bin).output().unwrap().stdout, b"replacement\n");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("corral-next");
        fs::copy(env::current_exe().unwrap(), &install).unwrap();
        let child = Command::new(install)
            .args(["--exact", TEST])
            .env(CHILD, "1")
            .env_remove("CORRAL_NEXT_BIN")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
    }
}

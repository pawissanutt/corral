//! `corral agent login`: one interactive owner login into the corral-owned Pi auth directory
//! (OD8), refreshed by Pi itself; never ~/.codex or another harness store.
//! Owner: T-252 child host-install.
//!
//! The directory is `<data root>/auth/` (0700) holding `auth.json` (0600), written only by the
//! installed `src/auth/` modules. corral itself reads only its metadata, never its bytes.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::Store;

use super::bridge::assets::{bun_version, find_bun, read_compat, version_satisfies};
use super::install::{data_root, installed_in};

/// The T-252 child that lands credential provisioning.
pub const OWNER: &str = "host-install";
pub const AUTH_FILE: &str = "auth.json";
pub const LOGIN_ENTRY: &str = "src/auth/login.ts";

pub fn auth_dir(root: &Path) -> PathBuf {
    root.join("auth")
}

/// Credential presence from file metadata only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStatus {
    pub dir: PathBuf,
    pub present: bool,
    /// Why the stored credential is unusable even though it exists (permissions, file type).
    pub problem: Option<String>,
}

#[cfg(unix)]
pub fn status_in(root: &Path) -> CredentialStatus {
    use std::os::unix::fs::PermissionsExt;
    let dir = auth_dir(root);
    let file = dir.join(AUTH_FILE);
    let mode = |meta: &std::fs::Metadata| meta.permissions().mode() & 0o777;
    let (present, problem) = match (
        std::fs::symlink_metadata(&dir),
        std::fs::symlink_metadata(&file),
    ) {
        (Err(_), _) | (_, Err(_)) => (false, None),
        (Ok(d), _) if !d.is_dir() || mode(&d) != 0o700 => (
            true,
            Some(format!("{} must be a 0700 directory", dir.display())),
        ),
        (_, Ok(f)) if !f.is_file() || mode(&f) & 0o077 != 0 => (
            true,
            Some(format!("{} must be a 0600 regular file", file.display())),
        ),
        (_, Ok(f)) => (f.len() > 0, None),
    };
    CredentialStatus {
        dir,
        present,
        problem,
    }
}

#[cfg(not(unix))]
pub fn status_in(root: &Path) -> CredentialStatus {
    CredentialStatus {
        dir: auth_dir(root),
        present: false,
        problem: Some("the Pi-native host runs on Unix only".into()),
    }
}

/// The assets whose real `src/auth/` runs the login: `[agent] assets_dir` when set, else the
/// managed installation. A fixture installation has only a synthetic credential and is refused.
pub fn login_assets(explicit: Option<PathBuf>, root: &Path) -> Result<PathBuf> {
    let assets = match explicit {
        Some(dir) => dir,
        None => installed_in(root)?.with_context(|| {
            format!(
                "assets_unavailable: no pi-native installation under {}; run `corral agent \
                 install` first",
                root.display()
            )
        })?,
    };
    if assets.join("fixture.json").exists() {
        bail!(
            "fixture_assets_not_allowed: {} is a fixture installation with a synthetic \
             credential; log in through the managed installation",
            assets.display()
        );
    }
    if !assets.join(LOGIN_ENTRY).is_file() {
        bail!(
            "credentials_unavailable: {} has no {LOGIN_ENTRY} (owner {OWNER})",
            assets.display()
        );
    }
    Ok(assets)
}

#[cfg(unix)]
pub fn ensure_auth_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).with_context(|| format!("creating {}", dir.display())),
    }
    let meta =
        std::fs::symlink_metadata(dir).with_context(|| format!("inspecting {}", dir.display()))?;
    if !meta.is_dir() {
        bail!(
            "{} must be a directory, not a symlink or file",
            dir.display()
        );
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting {} to 0700", dir.display()))
}

#[cfg(not(unix))]
pub fn ensure_auth_dir(_dir: &Path) -> Result<()> {
    bail!("the Pi-native host runs on Unix only")
}

/// Runs `bun --no-env-file <assets>/src/auth/login.ts --auth-dir <dir>` in this terminal: the
/// owner completes Pi's OpenAI Codex OAuth login and the credential lands in the auth directory.
pub fn login(start: &Path) -> Result<()> {
    let explicit = match Store::find_root(start) {
        Ok(root) => Store::open_unobserved(&root)?.config.agent.assets_dir,
        Err(_) => None,
    };
    let root = data_root()?;
    let assets = login_assets(explicit, &root)?;
    let range = read_compat(&assets)?.range;
    let bun = find_bun().context("bun_unavailable: no executable `bun` on PATH")?;
    let version = bun_version(&bun)?;
    if !version_satisfies(&version, &range) {
        bail!("bun_version_unsupported: bun {version} is outside the GO manifest's range {range}");
    }
    let dir = auth_dir(&root);
    ensure_auth_dir(&dir)?;
    let status = std::process::Command::new(&bun)
        .arg("--no-env-file")
        .arg(assets.join(LOGIN_ENTRY))
        .arg("--auth-dir")
        .arg(&dir)
        .current_dir(&assets)
        .status()
        .with_context(|| format!("running {}", bun.display()))?;
    if !status.success() {
        bail!("corral agent login did not complete ({status})");
    }
    let stored = status_in(&root);
    match (stored.present, stored.problem) {
        (true, None) => {
            println!("pi-native credentials present in {}", stored.dir.display());
            Ok(())
        }
        (_, Some(problem)) => bail!("credentials_unsafe: {problem}"),
        (false, None) => bail!(
            "credentials_unavailable: the login stored nothing in {}",
            stored.dir.display()
        ),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn status_reads_metadata_only_and_flags_loose_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!status_in(root).present);
        ensure_auth_dir(&auth_dir(root)).unwrap();
        let mode = std::fs::metadata(auth_dir(root))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        let file = auth_dir(root).join(AUTH_FILE);
        std::fs::write(&file, "{\"openai-codex\":{\"access\":\"SYNTHETIC-TOKEN\"}}").unwrap();
        chmod(&file, 0o644);
        let loose = status_in(root);
        assert!(loose.problem.unwrap().contains("0600"));
        chmod(&file, 0o600);
        assert_eq!(
            status_in(root),
            CredentialStatus {
                dir: auth_dir(root),
                present: true,
                problem: None
            }
        );
        chmod(&auth_dir(root), 0o755);
        assert!(status_in(root).problem.unwrap().contains("0700"));
        ensure_auth_dir(&auth_dir(root)).unwrap();
        assert!(status_in(root).problem.is_none());
    }

    #[test]
    fn login_refuses_a_fixture_or_missing_installation() {
        let tmp = tempfile::tempdir().unwrap();
        let none = login_assets(None, tmp.path()).unwrap_err().to_string();
        assert!(none.starts_with("assets_unavailable"), "{none}");
        let fixture = tmp.path().join("fixture");
        std::fs::create_dir_all(fixture.join("src/auth")).unwrap();
        std::fs::write(fixture.join(LOGIN_ENTRY), "").unwrap();
        std::fs::write(fixture.join("fixture.json"), "{}").unwrap();
        let refused = login_assets(Some(fixture.clone()), tmp.path())
            .unwrap_err()
            .to_string();
        assert!(
            refused.starts_with("fixture_assets_not_allowed"),
            "{refused}"
        );
        std::fs::remove_file(fixture.join("fixture.json")).unwrap();
        assert_eq!(
            login_assets(Some(fixture.clone()), tmp.path()).unwrap(),
            fixture
        );
    }
}

//! `corral agent login`: one interactive owner login into the corral-owned Pi auth directory
//! (OD8), refreshed by Pi itself; never ~/.codex, ~/.claude or another harness store.
//! Owner: T-252 child host-install; per-provider credentials and the picker: T-479.
//!
//! The directory is `<data root>/auth/` (0700) holding one `<provider>.json` (0600) per
//! provider, written only by the installed `src/auth/` modules. A Codex login from before T-479
//! is still the shared `auth.json` until its first refresh moves it. corral itself reads only
//! their metadata, never their bytes.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::Store;

use super::bridge::assets::{bun_version, find_bun, read_compat, version_satisfies};
use super::install::{data_root, installed_in};

#[path = "login_picker.rs"]
pub mod picker;

/// The T-252 child that lands credential provisioning.
pub const OWNER: &str = "host-install";
/// The pre-T-479 shared credential file; it only ever held the OpenAI Codex login.
pub const AUTH_FILE: &str = "auth.json";
pub const LOGIN_ENTRY: &str = "src/auth/login.ts";

/// A subscription OAuth provider pi-native can log in (`src/auth/providers.ts` lists the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub vendor: &'static str,
    pub label: &'static str,
}

pub const PROVIDERS: [Provider; 2] = [
    Provider {
        id: "openai-codex",
        vendor: "openai",
        label: "OpenAI Codex (ChatGPT Plus/Pro)",
    },
    Provider {
        id: "anthropic",
        vendor: "anthropic",
        label: "Anthropic Claude (Pro/Max)",
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// The provider whose credential a profile of `vendor` runs on.
pub fn provider_for_vendor(vendor: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.vendor == vendor)
}

/// `corral agent login` was cancelled in the picker; the CLI exits 130 and nothing was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginCancelled;

impl std::fmt::Display for LoginCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("corral agent login cancelled; no credential was written")
    }
}

impl std::error::Error for LoginCancelled {}

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

/// Whether any provider is logged in; a problem with any stored file is reported first.
pub fn status_in(root: &Path) -> CredentialStatus {
    let each: Vec<CredentialStatus> = PROVIDERS
        .iter()
        .map(|p| provider_status(root, p.id))
        .collect();
    CredentialStatus {
        dir: auth_dir(root),
        present: each.iter().any(|s| s.present),
        problem: each.into_iter().find_map(|s| s.problem),
    }
}

/// One provider's login state: its own file, else (Codex only) the pre-T-479 shared file.
pub fn provider_status(root: &Path, provider: &str) -> CredentialStatus {
    let own = file_status(root, &format!("{provider}.json"));
    if own.present || own.problem.is_some() || provider != PROVIDERS[0].id {
        return own;
    }
    file_status(root, AUTH_FILE)
}

#[cfg(unix)]
fn file_status(root: &Path, name: &str) -> CredentialStatus {
    use std::os::unix::fs::PermissionsExt;
    let dir = auth_dir(root);
    let file = dir.join(name);
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
fn file_status(root: &Path, _name: &str) -> CredentialStatus {
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

pub const USAGE: &str = "usage_error: `corral agent login` picks a provider interactively only on a \
     terminal; run `corral agent login --provider openai-codex` or `corral agent login --provider \
     anthropic`";

/// The provider to log in: `--provider` when given, else the owner's pick on a terminal. Without
/// a terminal it never prompts, so a scripted call cannot hang.
pub fn choose_provider(
    root: &Path,
    explicit: Option<&str>,
    terminal: bool,
    pick: impl FnOnce(&[picker::Row<'_>]) -> Result<Option<usize>>,
) -> Result<&'static Provider> {
    if let Some(id) = explicit {
        return provider(id).with_context(|| {
            format!("usage_error: unknown provider '{id}'; use openai-codex or anthropic")
        });
    }
    if !terminal {
        bail!(USAGE);
    }
    let rows: Vec<picker::Row<'_>> = PROVIDERS
        .iter()
        .map(|p| picker::Row {
            label: p.label,
            logged_in: provider_status(root, p.id).present,
        })
        .collect();
    match pick(&rows)? {
        Some(index) => Ok(&PROVIDERS[index]),
        None => Err(LoginCancelled.into()),
    }
}

#[cfg(unix)]
fn pick_on_terminal(rows: &[picker::Row<'_>]) -> Result<Option<usize>> {
    let mut tty = picker::Tty::open()?;
    let picked = picker::pick(&mut tty, &mut std::io::stderr(), rows);
    drop(tty);
    picked
}

#[cfg(not(unix))]
fn pick_on_terminal(_rows: &[picker::Row<'_>]) -> Result<Option<usize>> {
    bail!("the Pi-native host runs on Unix only")
}

/// Runs `bun --no-env-file <assets>/src/auth/login.ts --auth-dir <dir> --provider <id>` in this
/// terminal: the owner completes that provider's Pi OAuth login and the credential lands in its
/// file in the auth directory, replacing an earlier one atomically under the directory's lock.
pub fn login(start: &Path, provider: Option<&str>) -> Result<()> {
    use std::io::IsTerminal;
    let root = data_root()?;
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let chosen = choose_provider(&root, provider, terminal, pick_on_terminal)?;
    let explicit = match Store::find_root(start) {
        Ok(root) => Store::open_unobserved(&root)?.config.agent.assets_dir,
        Err(_) => None,
    };
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
        .arg("--provider")
        .arg(chosen.id)
        .current_dir(&assets)
        .status()
        .with_context(|| format!("running {}", bun.display()))?;
    if !status.success() {
        bail!("corral agent login did not complete ({status})");
    }
    let stored = provider_status(&root, chosen.id);
    match (stored.present, stored.problem) {
        (true, None) => {
            println!(
                "pi-native {} credentials present in {}",
                chosen.id,
                stored.dir.display()
            );
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

    fn credential(root: &Path, name: &str) {
        ensure_auth_dir(&auth_dir(root)).unwrap();
        let file = auth_dir(root).join(name);
        std::fs::write(&file, "{\"type\":\"oauth\"}").unwrap();
        chmod(&file, 0o600);
    }

    fn listing(root: &Path) -> Vec<String> {
        std::fs::read_dir(auth_dir(root))
            .map(|dir| {
                dir.map(|e| e.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn each_provider_has_its_own_state_and_a_pre_t479_auth_json_counts_for_codex() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!provider_status(root, "openai-codex").present);
        credential(root, AUTH_FILE);
        assert!(provider_status(root, "openai-codex").present);
        assert!(!provider_status(root, "anthropic").present);
        credential(root, "anthropic.json");
        assert!(provider_status(root, "anthropic").present);
        chmod(&auth_dir(root).join("anthropic.json"), 0o644);
        assert!(
            provider_status(root, "anthropic")
                .problem
                .unwrap()
                .contains("0600")
        );
        assert!(status_in(root).problem.is_some());
        assert_eq!(
            provider_for_vendor("anthropic").map(|p| p.id),
            Some("anthropic")
        );
        assert_eq!(
            provider_for_vendor("openai").map(|p| p.id),
            Some("openai-codex")
        );
    }

    #[test]
    fn the_terminal_picker_selects_anthropic_with_arrow_down_and_enter() {
        // Given a Codex login and a terminal sending arrow-down then Enter:
        let tmp = tempfile::tempdir().unwrap();
        credential(tmp.path(), "openai-codex.json");
        let mut keys: std::collections::VecDeque<u8> = b"\x1b[B\r".iter().copied().collect();
        let mut screen = Vec::new();
        // When login chooses its provider:
        let chosen = choose_provider(tmp.path(), None, true, |rows| {
            picker::pick(&mut keys, &mut screen, rows)
        })
        .unwrap();
        // Then Anthropic is selected and each row showed its login state from metadata:
        assert_eq!(chosen.id, "anthropic");
        let screen = String::from_utf8(screen).unwrap();
        assert!(
            screen.contains("> OpenAI Codex (ChatGPT Plus/Pro)   [logged in]"),
            "{screen}"
        );
        assert!(
            screen.contains("  Anthropic Claude (Pro/Max)        [not logged in]"),
            "{screen}"
        );
    }

    #[test]
    fn q_cancels_the_picker_with_login_cancelled_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut keys: std::collections::VecDeque<u8> = b"q".iter().copied().collect();
        let error = choose_provider(tmp.path(), None, true, |rows| {
            picker::pick(&mut keys, &mut std::io::sink(), rows)
        })
        .unwrap_err();
        assert!(error.is::<LoginCancelled>(), "{error:#}");
        assert!(!auth_dir(tmp.path()).exists());
        assert!(listing(tmp.path()).is_empty());
    }

    #[test]
    fn without_a_terminal_or_with_a_provider_flag_nothing_is_prompted() {
        let tmp = tempfile::tempdir().unwrap();
        let never = |_: &[picker::Row<'_>]| -> Result<Option<usize>> {
            panic!("the picker must not be shown")
        };
        let usage = choose_provider(tmp.path(), None, false, never).unwrap_err();
        assert!(
            usage.to_string().starts_with("usage_error:")
                && usage.to_string().contains("--provider"),
            "{usage}"
        );
        assert_eq!(
            choose_provider(tmp.path(), Some("anthropic"), false, never)
                .unwrap()
                .id,
            "anthropic"
        );
        assert!(
            choose_provider(tmp.path(), Some("mistral"), true, never)
                .unwrap_err()
                .to_string()
                .starts_with("usage_error: unknown provider")
        );
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

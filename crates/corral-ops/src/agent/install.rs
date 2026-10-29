//! `corral agent install`, `doctor` and `state`: the managed asset installation under
//! `$XDG_DATA_HOME/corral/pi-native/assets/<digest>/` and its readback.
//! Owner: T-252 child host-install.
//!
//! An installation copies only what a host runs (`package.json`, `bun.lock`, `compat.json`,
//! `src/` without `*.test.ts`, and `assets/`), never `test/` or `fixture.json`, runs
//! `bun install --frozen-lockfile` inside the copy and renames it to its asset digest. The
//! record `installed.json` names that digest; dispatch uses it when `[agent] assets_dir` is
//! unset, so a run never references the developer checkout it was installed from.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::agent::{
    AgentRunState, CapabilitySnapshot, CapabilityTransition, ContainerRef, pm_state,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bridge::assets::{
    HOST_BINARY, asset_digest, bun_version, find_bun, read_compat, version_satisfies,
};
use super::cli::{AgentTarget, InstallArgs, StateArgs};
use super::hooks::doctor;
use crate::Ctx;

/// The T-252 child that lands the installation.
pub const OWNER: &str = "host-install";
pub const RECORD_FILE: &str = "installed.json";
pub const RECORD_VERSION: u32 = 1;
const SHIPPED_FILES: [&str; 3] = ["package.json", "bun.lock", "compat.json"];
const SHIPPED_TREES: [&str; 2] = ["src", "assets"];
const OUTPUT_TAIL_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    pub version: u32,
    pub digest: String,
    pub path: PathBuf,
    pub bun: String,
    pub pi: String,
    pub installed_at: jiff::Timestamp,
}

/// `$XDG_DATA_HOME/corral/pi-native` (an absolute XDG_DATA_HOME only), else
/// `$HOME/.local/share/corral/pi-native`; `src/auth/paths.ts` resolves the same directory.
pub fn data_root() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(root) = super::test_support::data_root_override() {
        return Ok(root);
    }
    data_root_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

fn data_root_from(xdg: Option<OsString>, home: Option<OsString>) -> Result<PathBuf> {
    let absolute = |value: Option<OsString>| value.map(PathBuf::from).filter(|p| p.is_absolute());
    match (absolute(xdg), absolute(home)) {
        (Some(data), _) => Ok(data.join("corral/pi-native")),
        (None, Some(home)) => Ok(home.join(".local/share/corral/pi-native")),
        (None, None) => bail!("neither XDG_DATA_HOME nor HOME is an absolute path"),
    }
}

pub fn install(_start: &Path, args: &InstallArgs) -> Result<()> {
    let root = data_root()?;
    let record = match &args.from {
        Some(host) if host.is_file() => install_compiled(host, &root)?,
        Some(dir) => install_into(dir, &root, find_bun())?,
        None => match sibling_host() {
            Some(host) => install_compiled(&host, &root)?,
            None => install_into(&default_source()?, &root, find_bun())?,
        },
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!(
            "Installed pi-native assets sha256 {} at {} (bun {}, pi {})",
            record.digest,
            record.path.display(),
            record.bun,
            record.pi
        );
    }
    Ok(())
}

/// The compiled host a release archive installs beside the `corral` executable.
fn sibling_host() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let host = exe.parent()?.join(HOST_BINARY);
    host.is_file().then_some(host)
}

fn default_source() -> Result<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/corral-pi");
    if !dir.join("compat.json").is_file() {
        bail!(
            "no corral-pi package at {}; pass `--from <checkout>/packages/corral-pi`",
            dir.display()
        );
    }
    Ok(dir)
}

pub fn install_into(source: &Path, root: &Path, bun: Option<PathBuf>) -> Result<InstallRecord> {
    let source = source
        .canonicalize()
        .with_context(|| format!("resolving {}", source.display()))?;
    if source.join("fixture.json").exists() {
        bail!(
            "fixture_assets_not_allowed: {} is a fixture installation; fixtures load only through \
             an explicit [agent] assets_dir",
            source.display()
        );
    }
    let range = read_compat(&source)?.range;
    let pi = compat_pi_version(&source)?;
    let bun = bun.context("bun_unavailable: no executable `bun` on PATH")?;
    let bun_version = bun_version(&bun)?;
    if !version_satisfies(&bun_version, &range) {
        bail!(
            "bun_version_unsupported: bun {bun_version} at {} is outside the GO manifest's tested \
             range {range}",
            bun.display()
        );
    }
    let assets_root = root.join("assets");
    std::fs::create_dir_all(&assets_root)
        .with_context(|| format!("creating {}", assets_root.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(&assets_root)
        .with_context(|| format!("staging under {}", assets_root.display()))?;
    copy_shipped(&source, staging.path())?;
    let digest = asset_digest(staging.path())?;
    if !assets_root.join(&digest).exists() {
        bun_install(&bun, staging.path())?;
        if asset_digest(staging.path())? != digest {
            bail!("bun install changed the shipped asset files; refusing the installation");
        }
    }
    publish(root, staging.path(), digest, bun_version, pi)
}

/// Installs a compiled `corral-pi-host`: a copy of the executable plus the `compat.json` and
/// role assets it writes beside itself, so no Bun, checkout or `bun install` is involved.
pub fn install_compiled(host: &Path, root: &Path) -> Result<InstallRecord> {
    let host = host
        .canonicalize()
        .with_context(|| format!("resolving {}", host.display()))?;
    let assets_root = root.join("assets");
    std::fs::create_dir_all(&assets_root)
        .with_context(|| format!("creating {}", assets_root.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(&assets_root)
        .with_context(|| format!("staging under {}", assets_root.display()))?;
    // The source runs, never the fresh copy: a copy executed while another thread of this
    // process may hold its write descriptor across a fork fails with ETXTBSY.
    let version = run_host(&host, &["--version".as_ref()])?;
    let (pi, bun) = parse_host_version(&version).with_context(|| {
        format!(
            "{} --version printed {version:?}, not `corral-pi-host pi <v> bun <v>`",
            host.display()
        )
    })?;
    run_host(
        &host,
        &["--write-install".as_ref(), staging.path().as_os_str()],
    )?;
    let staged = staging.path().join(HOST_BINARY);
    copy_file(&host, &staged)?;
    set_executable(&staged)?;
    read_compat(staging.path())?;
    let digest = asset_digest(staging.path())?;
    publish(root, staging.path(), digest, bun, pi)
}

/// `corral-pi-host pi <pi> bun <bun>` -> `(pi, bun)`.
fn parse_host_version(text: &str) -> Option<(String, String)> {
    match text.split_whitespace().collect::<Vec<_>>()[..] {
        ["corral-pi-host", "pi", pi, "bun", bun] => Some((pi.to_owned(), bun.to_owned())),
        _ => None,
    }
}

fn run_host(host: &Path, args: &[&std::ffi::OsStr]) -> Result<String> {
    let output = std::process::Command::new(host)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {}", host.display()))?;
    if !output.status.success() {
        bail!(
            "{} {:?} failed ({}): {}",
            host.display(),
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Moves a staged installation to `assets/<digest>/` (or reuses an identical one) and records it.
fn publish(
    root: &Path,
    staging: &Path,
    digest: String,
    bun: String,
    pi: String,
) -> Result<InstallRecord> {
    let target = root.join("assets").join(&digest);
    if target.exists() {
        if asset_digest(&target).ok().as_deref() != Some(digest.as_str()) {
            bail!(
                "installed_assets_changed: {} exists with different content; remove it and \
                 reinstall",
                target.display()
            );
        }
    } else {
        std::fs::rename(staging, &target)
            .with_context(|| format!("installing {}", target.display()))?;
    }
    let record = InstallRecord {
        version: RECORD_VERSION,
        digest,
        path: target,
        bun,
        pi,
        installed_at: jiff::Timestamp::now(),
    };
    let staged = root.join(format!("{RECORD_FILE}.{}", std::process::id()));
    std::fs::write(&staged, serde_json::to_vec_pretty(&record)?)
        .with_context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, root.join(RECORD_FILE))
        .with_context(|| format!("recording {}", root.join(RECORD_FILE).display()))?;
    Ok(record)
}

pub(crate) fn compat_pi_version(source: &Path) -> Result<String> {
    let path = source.join("compat.json");
    let value: Value = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("parsing {}", path.display()))?;
    value["pi"]["version"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("{} names no pi.version", path.display()))
}

fn copy_shipped(source: &Path, target: &Path) -> Result<()> {
    for name in SHIPPED_FILES {
        copy_file(&source.join(name), &target.join(name))?;
    }
    for tree in SHIPPED_TREES {
        if source.join(tree).exists() {
            copy_tree(&source.join(tree), &target.join(tree))?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(from)
        .with_context(|| format!("inspecting {}", from.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular file", from.display());
    }
    std::fs::copy(from, to)
        .map(drop)
        .with_context(|| format!("copying {}", from.display()))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("listing {}", from.display()))? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        let name = entry.file_name();
        if kind.is_dir() {
            copy_tree(&path, &to.join(&name))?;
        } else if !name.to_string_lossy().ends_with(".test.ts") {
            copy_file(&path, &to.join(&name))?;
        }
    }
    Ok(())
}

fn bun_install(bun: &Path, dir: &Path) -> Result<()> {
    let output = std::process::Command::new(bun)
        .args(["install", "--frozen-lockfile"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {} install", bun.display()))?;
    if !output.status.success() {
        let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        let tail = &text[text.floor_char_boundary(text.len().saturating_sub(OUTPUT_TAIL_BYTES))..];
        bail!(
            "`bun install --frozen-lockfile` failed ({}):\n{tail}",
            output.status
        );
    }
    Ok(())
}

pub fn read_record(root: &Path) -> Result<Option<InstallRecord>> {
    let path = root.join(RECORD_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// The recorded installation under `root`, refused when its files changed since install.
pub fn installed_in(root: &Path) -> Result<Option<PathBuf>> {
    let Some(record) = read_record(root)? else {
        return Ok(None);
    };
    let dir = root.join("assets").join(&record.digest);
    if record.path != dir || !dir.is_dir() {
        bail!(
            "installed_assets_missing: {} is not the recorded installation; run `corral agent \
             install`",
            record.path.display()
        );
    }
    let now = asset_digest(&dir)?;
    if now != record.digest {
        bail!(
            "installed_assets_changed: {} has sha256 {now}, not the recorded {}; run `corral agent \
             install`",
            dir.display(),
            record.digest
        );
    }
    Ok(Some(dir))
}

/// The managed installation dispatch uses when `[agent] assets_dir` is unset. `None` until
/// `corral agent install` records one, so a pi-native dispatch refuses `assets_unavailable`.
pub fn installed_assets(_ctx: &Ctx) -> Result<Option<PathBuf>> {
    installed_in(&data_root()?)
}

pub fn doctor(ctx: &Ctx, json: bool) -> Result<()> {
    let checks = doctor::checks(ctx)?;
    if json {
        let lines: Vec<Value> = checks
            .iter()
            .map(|c| json!({"id": c.id, "ok": c.ok, "message": c.message, "fix": c.fix}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&lines)?);
    } else {
        for c in &checks {
            println!(
                "{} {}: {}",
                if c.ok { "ok  " } else { "FAIL" },
                c.id,
                c.message
            );
            if let Some(fix) = &c.fix {
                println!("      {fix}");
            }
        }
    }
    Ok(())
}

pub fn state(ctx: &Ctx, args: &StateArgs) -> Result<()> {
    let value = state_value(
        ctx,
        &args.target,
        &doctor::installation(ctx, data_root().ok()),
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!(
        "container: {}",
        value["container"]["key"].as_str().unwrap_or("")
    );
    for (label, field) in [
        ("harness", "harness"),
        ("role", "role"),
        ("health", "health"),
        ("generation", "generation"),
        ("generations", "generations"),
        ("receipts", "receipts"),
        ("open questions", "open_questions"),
        ("turns", "turns"),
        ("todo enabled", "todo_enabled"),
    ] {
        let item = &value["container"][field];
        if !item.is_null() {
            println!(
                "{label}: {}",
                item.as_str().map_or(item.to_string(), str::to_owned)
            );
        }
    }
    if let Some(todo) = value["container"]
        .get("todo")
        .filter(|todo| !todo.is_null())
    {
        println!("todo: {}", serde_json::to_string_pretty(todo)?);
    }
    let field = |name: &str| value["container"][name].clone();
    if let Ok(snapshot) = serde_json::from_value::<CapabilitySnapshot>(field("capabilities")) {
        println!("capabilities: {}", snapshot.summary());
    }
    if let Ok(change) = serde_json::from_value::<CapabilityTransition>(field("capability_change")) {
        println!("capability changes {}", change.summary());
    }
    println!(
        "assets: {}",
        value["installation"]["summary"].as_str().unwrap_or("")
    );
    Ok(())
}

pub(crate) fn state_value(ctx: &Ctx, target: &AgentTarget, installation: &Value) -> Result<Value> {
    let (key, harness, host) = match target {
        AgentTarget::Task { task, run, review } => {
            let record = ctx.store.get(task)?;
            let worker = match run {
                Some(id) => record.runs.iter().find(|r| &r.id == id),
                None => record.current_run(),
            }
            .with_context(|| format!("{task} has no run {}", run.as_deref().unwrap_or("")))?;
            let (container, run) = match review {
                Some(id) => (
                    ContainerRef::Reviewer {
                        task: task.clone(),
                        run: worker.id.clone(),
                        review: id.clone(),
                    },
                    worker
                        .review_runs
                        .iter()
                        .find(|c| &c.id == id)
                        .with_context(|| format!("{task}/{} has no review {id}", worker.id))?,
                ),
                None => (
                    ContainerRef::Worker {
                        task: task.clone(),
                        run: worker.id.clone(),
                    },
                    worker,
                ),
            };
            (
                container.key(),
                run.harness.clone(),
                run.agent_host.as_deref().cloned(),
            )
        }
        AgentTarget::Pm => (
            ContainerRef::Pm.key(),
            None,
            pm_state::load(&ctx.store)?.map(|pm| pm.state),
        ),
    };
    let mut container = json!({"key": key, "harness": harness});
    if matches!(target, AgentTarget::Pm)
        && pm_state::load(&ctx.store)?.is_some_and(|pm| pm.adopted())
    {
        container["adopted"] = Value::Bool(true);
    }
    if let Some(state) = &host {
        container
            .as_object_mut()
            .context("container is an object")?
            .extend(summary(state));
    }
    Ok(json!({"container": container, "installation": installation}))
}

fn summary(state: &AgentRunState) -> serde_json::Map<String, Value> {
    let open: Vec<&str> = state
        .questions
        .iter()
        .filter(|q| q.recorded_pending())
        .map(|q| q.key.as_str())
        .collect();
    let value = json!({
        "role": state.binding.role.as_str(),
        "health": state.binding.health,
        "generation": state.binding.generation,
        "pane": state.current_generation().map(|g| &g.pane),
        "bound": state.current_generation().is_some_and(|g| g.bound.is_some()),
        "generations": state.generations.len(),
        "receipts": state.receipts.len(),
        "open_questions": open,
        "turns": state.turns.len(),
        "todo_enabled": state.current_generation().is_some_and(|g| g.todo),
        "todo": state.todo,
        "capabilities": state.current_generation().map(|g| g.effective_capabilities(state.binding.role)),
        "capability_change": state.current_generation().and_then(|g| g.capability_transition.as_ref()),
    });
    match value {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use corral_core::agent::AgentRole;

    use super::*;
    use crate::agent::hooks::preflight;

    pub(crate) fn package() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/corral-pi")
    }

    /// A stand-in `bun` that reports `version` and, for `install`, records its argv in
    /// `node_modules/` and exits with `install_exit`. A child process writes the executable,
    /// so no write descriptor of this multi-threaded test process can make its exec fail
    /// with ETXTBSY.
    pub(crate) fn fake_bun(dir: &Path, version: &str, install_exit: i32) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let bun = dir.join("bun");
        let source = dir.join("bun.sh");
        std::fs::write(
            &source,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n--version) echo {version} ;;\ninstall) mkdir -p \
                 node_modules && echo \"$*\" > node_modules/.fake-install; exit {install_exit} ;;\n\
                 *) exit 9 ;;\nesac\n"
            ),
        )
        .unwrap();
        let installed = std::process::Command::new("install")
            .arg("-m")
            .arg("0755")
            .arg(&source)
            .arg(&bun)
            .status()
            .unwrap();
        assert!(installed.success());
        bun
    }

    fn digests(root: &Path) -> Vec<String> {
        std::fs::read_dir(root.join("assets"))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .filter(|name| !name.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_data_root_follows_xdg_then_home() {
        let root = |xdg: Option<&str>, home: Option<&str>| {
            data_root_from(xdg.map(Into::into), home.map(Into::into))
        };
        assert_eq!(
            root(Some("/data"), Some("/home/u")).unwrap(),
            Path::new("/data/corral/pi-native")
        );
        assert_eq!(
            root(Some("rel"), Some("/home/u")).unwrap(),
            Path::new("/home/u/.local/share/corral/pi-native")
        );
        assert!(root(None, None).is_err());
    }

    #[test]
    fn install_ships_only_host_files_and_dispatch_refuses_until_each_role_lands() {
        // Given the real package and a Bun inside its GO range:
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let bun = fake_bun(tmp.path(), "1.4.2", 0);

        // When it is installed:
        let record = install_into(&package(), &root, Some(bun.clone())).unwrap();

        // Then the copy is named by its digest and holds only what a host runs:
        let dir = root.join("assets").join(&record.digest);
        assert_eq!(record.path, dir);
        assert_eq!(asset_digest(&dir).unwrap(), record.digest);
        for shipped in [
            "package.json",
            "bun.lock",
            "compat.json",
            "src/host/main.ts",
            "src/auth/index.ts",
            "src/auth/login.ts",
        ] {
            assert!(dir.join(shipped).is_file(), "{shipped} missing");
        }
        for absent in ["test", "fixture.json", "src/auth/auth.test.ts"] {
            assert!(!dir.join(absent).exists(), "{absent} was installed");
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("node_modules/.fake-install")).unwrap(),
            "install --frozen-lockfile\n"
        );
        let recorded = std::fs::read_to_string(root.join(RECORD_FILE)).unwrap();
        let checkout = package().canonicalize().unwrap();
        assert!(!recorded.contains(checkout.to_str().unwrap()), "{recorded}");
        assert_eq!(
            (record.bun.as_str(), record.pi.as_str()),
            ("1.4.2", "1.3.0")
        );

        // And dispatch resolves all three landed production roles:
        assert_eq!(installed_in(&root).unwrap(), Some(dir.clone()));
        preflight::assets(&dir, AgentRole::Worker, false).unwrap();
        preflight::assets(&dir, AgentRole::Reviewer, false).unwrap();
        preflight::assets(&dir, AgentRole::Pm, false).unwrap();

        // And a reinstall of the same package is the same installation:
        let again = install_into(&package(), &root, Some(bun)).unwrap();
        assert_eq!(again.digest, record.digest);
        assert_eq!(digests(&root), vec![record.digest.clone()]);

        // A genuinely missing PM role still names its owner and is refused.
        std::fs::remove_file(dir.join("src/roles/pm/index.ts")).unwrap();
        let refusal = preflight::assets(&dir, AgentRole::Pm, false).unwrap_err();
        assert_eq!(refusal.code, "role_unavailable");
        assert!(refusal.detail.starts_with("pm, owner pm-role"), "{refusal}");
    }

    #[test]
    fn a_changed_installation_is_refused_rather_than_dispatched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let record =
            install_into(&package(), &root, Some(fake_bun(tmp.path(), "1.4.2", 0))).unwrap();
        std::fs::write(
            record.path.join("src/auth/index.ts"),
            "export default {};\n",
        )
        .unwrap();
        let error = installed_in(&root).unwrap_err().to_string();
        assert!(error.starts_with("installed_assets_changed"), "{error}");
    }

    #[test]
    fn install_refuses_fixtures_unsupported_bun_and_a_failed_bun_install_leaving_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let refused = |source: &Path, bun: Option<PathBuf>| {
            install_into(source, &root, bun).unwrap_err().to_string()
        };
        let fixture = tmp.path().join("fixture");
        std::fs::create_dir_all(&fixture).unwrap();
        std::fs::write(fixture.join("fixture.json"), "{}").unwrap();
        assert!(refused(&fixture, None).starts_with("fixture_assets_not_allowed"));
        assert!(refused(&package(), None).starts_with("bun_unavailable"));
        let old = fake_bun(&tmp.path().join("old"), "1.3.0", 0);
        assert!(refused(&package(), Some(old)).starts_with("bun_version_unsupported"));
        let failing = fake_bun(&tmp.path().join("failing"), "1.4.2", 3);
        assert!(refused(&package(), Some(failing)).contains("bun install --frozen-lockfile"));
        assert!(digests(&root).is_empty());
        assert!(read_record(&root).unwrap().is_none());
    }

    /// A stand-in compiled host: `--version` and `--write-install <dir>` as src/host/compiled.ts
    /// answers them, writing the package's real compat.json.
    fn fake_host(dir: &Path, version: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let source = dir.join("host.sh");
        let compat = package().join("compat.json");
        std::fs::write(
            &source,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n--version) echo '{version}' ;;\n--write-install) mkdir -p \
                 \"$2/assets\" && cp '{}' \"$2/compat.json\" && echo contract > \"$2/assets/worker-contract.md\" ;;\n\
                 *) exit 9 ;;\nesac\n",
                compat.display()
            ),
        )
        .unwrap();
        let host = dir.join(HOST_BINARY);
        let installed = std::process::Command::new("install")
            .args(["-m", "0755"])
            .arg(&source)
            .arg(&host)
            .status()
            .unwrap();
        assert!(installed.success());
        host
    }

    #[test]
    fn a_compiled_host_installs_without_bun_and_serves_every_role() {
        // Given a compiled host and no Bun anywhere:
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let host = fake_host(
            &tmp.path().join("release"),
            "corral-pi-host pi 1.3.0 bun 1.4.2",
        );

        // When it is installed:
        let record = install_compiled(&host, &root).unwrap();

        // Then the installation is the executable plus what it wrote, named by its digest:
        let dir = root.join("assets").join(&record.digest);
        assert_eq!(record.path, dir);
        assert_eq!(
            (record.bun.as_str(), record.pi.as_str()),
            ("1.4.2", "1.3.0")
        );
        assert!(dir.join(HOST_BINARY).is_file() && dir.join("compat.json").is_file());
        assert!(!dir.join("src/host/main.ts").exists() && !dir.join("node_modules").exists());
        assert_eq!(installed_in(&root).unwrap(), Some(dir.clone()));

        // And every role is served by the bundle, with no source modules on disk:
        for role in [AgentRole::Worker, AgentRole::Reviewer, AgentRole::Pm] {
            preflight::assets(&dir, role, false).unwrap();
        }

        // And reinstalling the same executable is the same installation:
        assert_eq!(
            install_compiled(&host, &root).unwrap().digest,
            record.digest
        );
        assert_eq!(digests(&root), vec![record.digest.clone()]);

        // And a replaced executable is refused rather than launched:
        std::fs::write(dir.join(HOST_BINARY), "#!/bin/sh\nexit 0\n").unwrap();
        assert!(
            installed_in(&root)
                .unwrap_err()
                .to_string()
                .starts_with("installed_assets_changed")
        );
    }

    #[test]
    fn a_host_that_does_not_identify_itself_is_refused_leaving_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let host = fake_host(&tmp.path().join("release"), "something else");
        let error = install_compiled(&host, &root).unwrap_err().to_string();
        assert!(
            error.contains("not `corral-pi-host pi <v> bun <v>`"),
            "{error}"
        );
        assert!(digests(&root).is_empty());
        assert!(read_record(&root).unwrap().is_none());
        assert_eq!(
            parse_host_version("corral-pi-host pi 1.3.0 bun 1.4.2"),
            Some(("1.3.0".to_owned(), "1.4.2".to_owned()))
        );
    }
}

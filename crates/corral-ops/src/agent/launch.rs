//! `corral agent launch --bootstrap <path> [--assets <dir>]`: the Rust launcher run in the
//! pane. It consumes the one-shot bootstrap, checks the asset digest, Bun range, GO manifest and
//! the role, delivery and auth modules, generates `host_cap` and `ctl_cap`, writes `ctl.cap` and
//! execs Bun by argv, passing the launch message (nonce, scope and both secrets) on an
//! inherited pipe: never argv, the environment or a file the model can read. Owner: T-252
//! child host-bridge. `prepare` is the dispatch half: it opens the generation, writes the
//! bootstrap and renders the pane command.
//!
//! The runtime-directory layout lives here so run-end cleanup (integration-seams) and the
//! launcher agree on it: `$XDG_RUNTIME_DIR/corral/<store>/<recipient>/`, else an owner-only
//! `/tmp/corral-<uid>/<store>/<recipient>/`, with one `g<n>/` per generation below it.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::agent::{self, AgentRole, AgentRunState, ContainerRef, paths, pm_state};
use corral_core::inbox::Recipient;
use corral_core::{SessionAction, SessionPolicy, Store};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::admission::{BindTarget, open_generation};
use super::bridge::assets::{
    asset_digest, bun_version, find_bun, missing_modules, read_compat, version_satisfies,
};
use super::cli::LaunchArgs;
use crate::Ctx;

/// The T-252 child that lands the launcher and the launch command.
pub const OWNER: &str = "host-bridge";
pub const BOOTSTRAP_VERSION: u32 = 1;
pub const BOOTSTRAP_FILE: &str = "bootstrap.json";
pub const CTL_CAP_FILE: &str = "ctl.cap";
pub const SOCKET_FILE: &str = "host.sock";
/// The host entry point under an asset root.
pub const HOST_ENTRY: &str = "src/host/main.ts";
const BOOTSTRAP_MAX_BYTES: u64 = 64 * 1024;
const MESSAGE_MAX_BYTES: usize = 32 * 1024;

/// The one-shot bootstrap dispatch writes 0600 into the generation's 0700 runtime directory.
/// It holds the bind nonce, so its `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub version: u32,
    /// The workspace root; the host runs every `corral agent call` there.
    pub root: PathBuf,
    pub store: String,
    pub project: String,
    pub role: AgentRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    pub generation: u32,
    pub bind_nonce: String,
    pub pane: String,
    pub cwd: PathBuf,
    pub assets: PathBuf,
    pub asset_sha256: String,
    pub corral: PathBuf,
    pub session_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session: Option<PathBuf>,
    #[serde(default)]
    pub fresh_session: bool,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<PathBuf>,
}

impl std::fmt::Debug for Bootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bootstrap")
            .field("store", &self.store)
            .field("role", &self.role)
            .field("task", &self.task)
            .field("run", &self.run)
            .field("generation", &self.generation)
            .field("bind_nonce", &"<redacted>")
            .field("assets", &self.assets)
            .finish_non_exhaustive()
    }
}

impl Bootstrap {
    pub fn recipient(&self) -> Result<Recipient> {
        let task = || self.task.clone().context("the bootstrap names no task");
        let run = || self.run.clone().context("the bootstrap names no run");
        Ok(match self.role {
            AgentRole::Worker => Recipient::Run {
                task: task()?,
                run: run()?,
            },
            AgentRole::Reviewer => Recipient::ReviewRun {
                task: task()?,
                run: run()?,
                review: self
                    .review
                    .clone()
                    .context("the bootstrap names no review")?,
            },
            AgentRole::Pm => Recipient::Pm,
        })
    }
}

/// A launch refused before exec: a stable code and what is wrong. No secret exists yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for LaunchRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "corral agent launch refused: {}: {}",
            self.code, self.detail
        )
    }
}

impl std::error::Error for LaunchRefusal {}

fn refuse(code: &'static str, detail: impl Into<String>) -> LaunchRefusal {
    LaunchRefusal {
        code,
        detail: detail.into(),
    }
}

/// Runs in the pane: consumes the bootstrap, checks everything, then execs the Bun host.
#[cfg(unix)]
pub fn launch(_start: &Path, args: &LaunchArgs) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let bootstrap = consume_bootstrap(&args.bootstrap)?;
    let generation_dir = args
        .bootstrap
        .parent()
        .context("the bootstrap has no directory")?;
    let ready = check(&bootstrap, generation_dir, args.assets.as_deref())?;
    let secrets = Secrets::generate()?;
    write_ctl_cap(&ready.generation_dir, &secrets.ctl_cap)?;
    let (mut command, _inherited) = host_command(&bootstrap, &ready, &secrets)?;
    let error = command.exec();
    Err(error).with_context(|| format!("executing {}", ready.bun.display()))
}

#[cfg(not(unix))]
pub fn launch(_start: &Path, _args: &LaunchArgs) -> Result<()> {
    bail!("corral agent launch refused: unsupported_platform: the Pi-native host runs on Unix")
}

/// The pane command for a pi-native reviewer or PM: their launches land with reviewer-role
/// and pm-role, which call `prepare`.
pub fn command(recipient: &Recipient) -> Result<String> {
    let owner = match recipient {
        Recipient::ReviewRun { .. } => "reviewer-role",
        Recipient::Pm => "pm-role",
        _ => "worker-role",
    };
    bail!(
        "op_unavailable: the pi-native launch command for {} lands with T-252 child {owner}",
        recipient.key()?
    )
}

/// What a container's host runs with, besides its bound identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    pub cwd: PathBuf,
    pub model: String,
    pub effort: Option<String>,
    pub prompt_file: Option<PathBuf>,
}

/// The dispatch half: opens generation n+1 of `target` in `pane`, writes its one-shot bootstrap
/// and returns the pane command `{corral} agent launch --bootstrap <path> [--assets <dir>]`.
pub fn prepare(ctx: &Ctx, target: &BindTarget, pane: &str, spec: &HostSpec) -> Result<String> {
    let explicit = ctx.store.config.agent.assets_dir.clone();
    let assets = match explicit.clone() {
        Some(dir) => dir,
        None => super::install::installed_assets(ctx)?.ok_or_else(|| {
            refuse(
                "assets_unavailable",
                format!(
                    "no pi-native asset installation (owner {})",
                    super::install::OWNER
                ),
            )
        })?,
    };
    let assets =
        std::path::absolute(&assets).with_context(|| format!("resolving {}", assets.display()))?;
    let asset_sha256 = asset_digest(&assets)?;
    let container = target.container();
    let mut session_dir = paths::container_sessions_dir(&ctx.store, &container);
    let mut resume_session = None;
    let mut fresh_session = false;
    let latest_bound = |state: &AgentRunState| {
        state
            .generations
            .iter()
            .filter_map(|generation| generation.bound.as_ref().map(|bound| (generation.n, bound)))
            .max_by_key(|(n, _)| *n)
            .map(|(_, bound)| bound.session_file.clone())
    };
    let (project, recovery) = match target {
        BindTarget::Pm { project } => (
            project.clone(),
            pm_state::load(&ctx.store)?.and_then(|pm| latest_bound(&pm.state)),
        ),
        BindTarget::Worker { .. } | BindTarget::Reviewer { .. } => {
            let task = ctx.store.get(
                container
                    .task()
                    .context("a worker or reviewer names a task")?,
            )?;
            let run = agent::container_run(&task, &container)
                .with_context(|| format!("{} does not exist", container.key()))?;
            if let Some(session) = &run.session {
                session_dir = session.directory.clone();
                fresh_session =
                    session.policy == SessionPolicy::Fresh || session.action == SessionAction::New;
                if !fresh_session
                    && matches!(
                        session.action,
                        SessionAction::Reused | SessionAction::Resumed
                    )
                {
                    resume_session = session.transcript.clone();
                }
            }
            (
                task.project
                    .clone()
                    .context("a Pi-native container needs a task project")?,
                run.agent_host.as_deref().and_then(latest_bound),
            )
        }
    };
    // A host recovery resumes its latest hello-bound file, not the session selected before
    // this container's first generation (which may belong to a previous review child).
    if let Some(file) =
        recovery.filter(|file| !fresh_session || file.parent() == Some(session_dir.as_path()))
    {
        fresh_session = false;
        session_dir = file
            .parent()
            .context("the bound Pi session file has no directory")?
            .to_path_buf();
        resume_session = Some(file);
    }
    let grant = open_generation(&ctx.store, target, pane)?;
    let (task, run, review) = match &container {
        ContainerRef::Worker { task, run } => (Some(task.clone()), Some(run.clone()), None),
        ContainerRef::Reviewer { task, run, review } => {
            (Some(task.clone()), Some(run.clone()), Some(review.clone()))
        }
        ContainerRef::Pm => (None, None, None),
    };
    let bootstrap = Bootstrap {
        version: BOOTSTRAP_VERSION,
        root: ctx.store.root().to_path_buf(),
        store: paths::store_id(&ctx.store),
        project,
        role: match target {
            BindTarget::Worker { .. } => AgentRole::Worker,
            BindTarget::Reviewer { .. } => AgentRole::Reviewer,
            BindTarget::Pm { .. } => AgentRole::Pm,
        },
        task,
        run,
        review,
        callback: match target {
            BindTarget::Reviewer { callback, .. } => Some(callback.clone()),
            _ => None,
        },
        generation: grant.generation,
        bind_nonce: grant.bind_nonce,
        pane: pane.into(),
        cwd: spec.cwd.clone(),
        assets: assets.clone(),
        asset_sha256,
        corral: crate::prompt::corral_bin()?,
        session_dir,
        resume_session,
        fresh_session,
        model: spec.model.clone(),
        effort: spec.effort.clone(),
        prompt_file: spec.prompt_file.clone(),
    };
    let dir = generation_dir(&ctx.store, &bootstrap.recipient()?, grant.generation)?;
    let path = write_bootstrap(&dir, &bootstrap)?;
    let quote = |p: &Path| {
        format!(
            "'{}'",
            crate::prompt::sh_escape(&crate::prompt::shell_path(p))
        )
    };
    let mut command = format!(
        "{} agent launch --bootstrap {}",
        quote(&bootstrap.corral),
        quote(&path)
    );
    if explicit.is_some() {
        command.push_str(&format!(" --assets {}", quote(&assets)));
    }
    Ok(command)
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
    container_dir(root, &paths::store_id(store), recipient)
}

fn container_dir(root: &Path, store_id: &str, recipient: &Recipient) -> Result<PathBuf> {
    let key = recipient.key()?;
    Ok(root
        .join(store_id)
        .join(&paths::sha256_hex(key.as_bytes())[..16]))
}

/// `g<n>/` of a container: its bootstrap, `ctl.cap` and `host.sock`.
pub fn generation_dir(store: &Store, recipient: &Recipient, generation: u32) -> Result<PathBuf> {
    Ok(runtime_dir(store, recipient)?.join(format!("g{generation}")))
}

#[cfg(unix)]
fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

#[cfg(not(unix))]
fn uid() -> u32 {
    0
}

/// Creates `dir` and every missing directory between it and the runtime root 0700, tightens a
/// looser one of this user's to 0700, and refuses a symlink or another user's directory.
#[cfg(unix)]
pub fn ensure_private_dir(root: &Path, dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let relative = dir
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", dir.display(), root.display()))?;
    let mut path = root.to_path_buf();
    let mut levels = vec![path.clone()];
    for part in relative.components() {
        path.push(part);
        levels.push(path.clone());
    }
    for level in levels {
        match std::fs::DirBuilder::new().mode(0o700).create(&level) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", level.display()));
            }
        }
        let meta = std::fs::symlink_metadata(&level)
            .with_context(|| format!("inspecting {}", level.display()))?;
        if meta.is_dir() && meta.uid() == uid() && meta.mode() & 0o077 != 0 {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&level, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("restricting {} to 0700", level.display()))?;
        }
        let meta = std::fs::symlink_metadata(&level)
            .with_context(|| format!("inspecting {}", level.display()))?;
        if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
            bail!(
                "unsafe runtime directory {}: it must be a 0700 directory owned by uid {}",
                level.display(),
                uid()
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", path.display()))
}

/// Writes the bootstrap 0600 with create-new into `dir` (made 0700 first).
#[cfg(unix)]
pub fn write_bootstrap(dir: &Path, bootstrap: &Bootstrap) -> Result<PathBuf> {
    ensure_private_dir(&runtime_root(), dir)?;
    let path = dir.join(BOOTSTRAP_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("clearing {}", path.display())),
    }
    write_private(&path, &serde_json::to_vec(bootstrap)?)?;
    Ok(path)
}

#[cfg(not(unix))]
pub fn write_bootstrap(_dir: &Path, _bootstrap: &Bootstrap) -> Result<PathBuf> {
    bail!("the Pi-native host runs on Unix only")
}

/// Reads and unlinks the bootstrap. It must be a 0600 regular file of this user inside the
/// 0700 generation directory it names, so a moved or foreign bootstrap is refused.
#[cfg(unix)]
pub fn consume_bootstrap(path: &Path) -> Result<Bootstrap> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let meta = std::fs::symlink_metadata(path).map_err(|error| {
        refuse(
            "bootstrap_unavailable",
            format!("{}: {error}", path.display()),
        )
    })?;
    if !meta.is_file() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
        return Err(refuse(
            "bootstrap_unsafe",
            format!(
                "{} must be a 0600 file owned by uid {}",
                path.display(),
                uid()
            ),
        )
        .into());
    }
    let mut bytes = Vec::new();
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .take(BOOTSTRAP_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    std::fs::remove_file(path).with_context(|| format!("consuming {}", path.display()))?;
    if bytes.len() as u64 > BOOTSTRAP_MAX_BYTES {
        return Err(refuse("bootstrap_invalid", "the bootstrap exceeds its bound").into());
    }
    let bootstrap: Bootstrap = serde_json::from_slice(&bytes).map_err(|error| {
        refuse(
            "bootstrap_invalid",
            format!(
                "unreadable bootstrap ({})",
                super::envelope::error_category(&error)
            ),
        )
    })?;
    if bootstrap.version != BOOTSTRAP_VERSION {
        return Err(refuse("bootstrap_invalid", "unsupported bootstrap version").into());
    }
    let tail = container_dir(Path::new(""), &bootstrap.store, &bootstrap.recipient()?)?
        .join(format!("g{}", bootstrap.generation));
    let dir = path.parent().unwrap_or(Path::new(""));
    let root = dir
        .ends_with(&tail)
        .then(|| dir.ancestors().nth(tail.components().count()))
        .flatten()
        .filter(|root| {
            root.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "corral" || name.starts_with("corral-"))
        })
        .ok_or_else(|| {
            refuse(
                "bootstrap_unsafe",
                "the bootstrap is not in its own generation's runtime directory",
            )
        })?;
    ensure_private_dir(root, dir)?;
    Ok(bootstrap)
}

/// Everything the exec needs once every check passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub assets: PathBuf,
    pub bun: PathBuf,
    pub bun_version: String,
    pub generation_dir: PathBuf,
}

/// The ordered pre-exec checks: assets, fixture rule, GO manifest, asset digest, Bun range and
/// the role, delivery and auth modules (every missing one is named with its owning child).
pub fn check(
    bootstrap: &Bootstrap,
    generation_dir: &Path,
    assets_flag: Option<&Path>,
) -> Result<Ready, LaunchRefusal> {
    check_with(bootstrap, generation_dir, assets_flag, find_bun())
}

/// `check` with the Bun found on PATH supplied by the caller.
pub fn check_with(
    bootstrap: &Bootstrap,
    generation_dir: &Path,
    assets_flag: Option<&Path>,
    bun: Option<PathBuf>,
) -> Result<Ready, LaunchRefusal> {
    let assets = assets_flag.unwrap_or(&bootstrap.assets).to_path_buf();
    if !assets.is_absolute() || !assets.is_dir() {
        return Err(refuse(
            "assets_unavailable",
            format!("{} is not an asset directory", assets.display()),
        ));
    }
    if assets.join("fixture.json").exists() && !explicit_assets(&bootstrap.root, &assets) {
        return Err(refuse(
            "fixture_assets_not_allowed",
            format!(
                "{} is a fixture installation; fixtures load only through an explicit [agent] \
                 assets_dir",
                assets.display()
            ),
        ));
    }
    let compat = read_compat(&assets)?;
    let digest = asset_digest(&assets)
        .map_err(|error| refuse("assets_unavailable", format!("{error:#}")))?;
    if digest != bootstrap.asset_sha256 {
        return Err(refuse(
            "asset_digest_changed",
            format!(
                "{} changed since dispatch (sha256 {} at dispatch, {digest} now)",
                assets.display(),
                bootstrap.asset_sha256
            ),
        ));
    }
    let bun = bun.ok_or_else(|| {
        refuse(
            "bun_unavailable",
            "no executable `bun` on PATH (owner host-install)",
        )
    })?;
    let bun_version = bun_version(&bun)?;
    if !version_satisfies(&bun_version, &compat.range) {
        return Err(refuse(
            "bun_version_unsupported",
            format!(
                "bun {bun_version} at {} is outside the GO manifest's tested range {}",
                bun.display(),
                compat.range
            ),
        ));
    }
    let missing = missing_modules(&assets, bootstrap.role);
    if let Some((code, _)) = missing.first() {
        let detail = missing
            .iter()
            .map(|(code, detail)| format!("{code} {detail}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(refuse(code, detail));
    }
    Ok(Ready {
        assets,
        bun,
        bun_version,
        generation_dir: generation_dir.to_path_buf(),
    })
}

/// A fixture is accepted only from the directory the store's `[agent] assets_dir` names.
fn explicit_assets(root: &Path, assets: &Path) -> bool {
    let Ok(store) = Store::open_unobserved(root) else {
        return false;
    };
    let canonical = |p: &Path| p.canonicalize().ok();
    store
        .config
        .agent
        .assets_dir
        .as_deref()
        .and_then(canonical)
        .is_some_and(|dir| Some(dir) == canonical(assets))
}

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

/// The JSON the host reads from its inherited descriptor (`src/host/config.ts`).
pub fn launch_message(bootstrap: &Bootstrap, ready: &Ready, secrets: &Secrets) -> Result<Vec<u8>> {
    let message = json!({
        "version": 1, "store": bootstrap.store, "project": bootstrap.project,
        "role": bootstrap.role.as_str(), "task": bootstrap.task, "run": bootstrap.run,
        "review": bootstrap.review, "callback": bootstrap.callback,
        "generation": bootstrap.generation,
        "secrets": {"host_cap": secrets.host_cap, "ctl_cap": secrets.ctl_cap,
                    "bind_nonce": bootstrap.bind_nonce},
        "pane": bootstrap.pane, "cwd": bootstrap.cwd, "root": bootstrap.root,
        "corral": bootstrap.corral, "assets": ready.assets,
        "runtime_dir": ready.generation_dir, "session_dir": bootstrap.session_dir,
        "resume_session": bootstrap.resume_session,
        "fresh_session": bootstrap.fresh_session,
        "model": bootstrap.model, "effort": bootstrap.effort, "prompt_file": bootstrap.prompt_file,
    });
    let bytes = serde_json::to_vec(&message)?;
    if bytes.len() > MESSAGE_MAX_BYTES {
        bail!("the launch message exceeds {MESSAGE_MAX_BYTES} bytes");
    }
    Ok(bytes)
}

/// `bun --no-env-file <assets>/src/host/main.ts --launch-fd <n>` in the asset directory, with
/// the launch message already written into a pipe whose read end the child inherits as `<n>`.
/// The returned reader must outlive the spawn or exec.
#[cfg(unix)]
pub fn host_command(
    bootstrap: &Bootstrap,
    ready: &Ready,
    secrets: &Secrets,
) -> Result<(std::process::Command, std::io::PipeReader)> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let (reader, mut writer) = std::io::pipe().context("creating the launch pipe")?;
    writer
        .write_all(&launch_message(bootstrap, ready, secrets)?)
        .context("writing the launch message")?;
    drop(writer);
    rustix::io::fcntl_setfd(&reader, rustix::io::FdFlags::empty())
        .context("letting the host inherit the launch pipe")?;
    let mut command = std::process::Command::new(&ready.bun);
    command
        .arg("--no-env-file")
        .arg(ready.assets.join(HOST_ENTRY))
        .arg("--launch-fd")
        .arg(reader.as_raw_fd().to_string())
        .current_dir(&ready.assets);
    Ok((command, reader))
}

#[cfg(all(test, unix))]
#[path = "bridge/launch_tests.rs"]
mod tests;

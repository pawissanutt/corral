//! Untethered build launches: read configuration and credentials without opening a store, and list
//! the build sessions the host recorded so the launcher can offer them for resume.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::AgentRole;
use corral_core::config::{BuildExit, Config};
use corral_core::{ConfigLayers, Store};
use serde_json::json;

use super::bridge::assets::{bun_version, compiled_host, find_bun, read_compat, version_satisfies};
use super::hooks::pm::Overrides;
use super::launch::capabilities_payload;
use super::pi::{DEFAULT_PROVIDER, platform, refuse};

const BUILD_ENTRY: &str = "src/host/build.ts";
const MESSAGE_MAX_BYTES: usize = 32 * 1024;

#[path = "pi_build_profiles.rs"]
mod profiles;
pub(crate) use profiles::mixed_api;
#[cfg(unix)]
#[path = "pi_build_supervisor.rs"]
mod supervisor;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildArgs {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
    pub here: bool,
    pub name: Option<String>,
    pub resume: Option<String>,
}

fn validate_args(args: &BuildArgs) -> Result<()> {
    if args.profile.is_some()
        && (args.model.is_some() || args.provider.is_some() || args.effort.is_some())
    {
        return Err(refuse(
            "usage_error",
            "--profile conflicts with --model, --provider and --effort",
        ));
    }
    if let Some(name) = &args.name {
        let bytes = name.as_bytes();
        if !(1..=40).contains(&bytes.len())
            || !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
            || !bytes
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        {
            return Err(refuse(
                "usage_error",
                "--name must match [a-z0-9][a-z0-9-]{0,39}",
            ));
        }
    }
    if let Some(resume) = &args.resume {
        if !resume.strip_prefix("B-").is_some_and(|id| {
            (6..=16).contains(&id.len())
                && id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        }) {
            return Err(refuse(
                "usage_error",
                "--resume must match B-[a-z0-9]{6,16}",
            ));
        }
        if args.here || args.name.is_some() {
            return Err(refuse(
                "usage_error",
                "--resume conflicts with --here and --name",
            ));
        }
    }
    if let Some(provider) = &args.provider {
        require_provider(provider)?;
    }
    Ok(())
}

fn require_provider(id: &str) -> Result<&'static super::credentials::Provider> {
    super::credentials::provider(id)
        .ok_or_else(|| refuse("usage_error", format!("unknown provider {id}")))
}

fn model(
    config: &Config,
    workspace: bool,
    args: &BuildArgs,
) -> Result<(String, String, Option<String>)> {
    if workspace || args.profile.is_some() {
        let name = args
            .profile
            .as_ref()
            .or(config.assistant.agent.as_ref())
            .unwrap_or(&config.pm.agent);
        let route = profiles::resolve(config, name, args)?;
        return Ok((route.provider, route.model, route.effort));
    }
    let provider = args.provider.as_deref().unwrap_or(DEFAULT_PROVIDER);
    require_provider(provider)?;
    let default = match provider {
        "anthropic" => "claude-opus-4-8",
        "amazon-bedrock" => "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
        _ => "gpt-6.1-sol",
    };
    Ok((
        provider.into(),
        args.model.as_deref().unwrap_or(default).into(),
        args.effort.clone(),
    ))
}

fn bare_model(args: &BuildArgs) -> Result<(String, String, Option<String>)> {
    let provider = args.provider.as_deref().unwrap_or(DEFAULT_PROVIDER);
    let default = match provider {
        "openai-codex" => "gpt-6.1-sol",
        "anthropic" => "claude-opus-4-8",
        "amazon-bedrock" => "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
        "openai" => {
            return Err(refuse(
                "usage_error",
                "API connections require bound canonical metered admission",
            ));
        }
        "llama.cpp" => {
            return Err(refuse(
                "usage_error",
                "llama.cpp requires a bound role's configured llama_cpp capability",
            ));
        }
        _ => {
            return Err(refuse(
                "usage_error",
                format!("unknown provider {provider}"),
            ));
        }
    };
    Ok((
        provider.into(),
        args.model.as_deref().unwrap_or(default).into(),
        args.effort.clone(),
    ))
}

fn workspace_for(path: &Path) -> Result<Option<PathBuf>> {
    match Store::find_root(path) {
        Ok(root) => Ok(Some(root)),
        Err(error) if error.to_string() == "no .corral workspace found (run `corral init`)" => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn resume_session(data_root: &Path, id: &str) -> Result<BuildSessionSummary> {
    let (_, session) = read_session(&data_root.join("build").join(id)).ok_or_else(|| {
        refuse(
            "build_session_unknown",
            format!("cannot read build session {id}"),
        )
    })?;
    if !session.repo_root.is_dir() {
        return Err(refuse(
            "build_session_unknown",
            format!(
                "build session {id} repository {} no longer exists",
                session.repo_root.display()
            ),
        ));
    }
    Ok(session)
}

/// Prepares the host and its inherited launch pipe without writing any workspace state.
#[cfg(unix)]
pub fn build_command(
    data_root: &Path,
    args: &BuildArgs,
    cwd: &Path,
    bun: Option<PathBuf>,
    workspace: Option<&Path>,
) -> Result<(std::process::Command, std::io::PipeReader)> {
    command(data_root, args, cwd, bun, workspace, false, None)
}

#[cfg(unix)]
pub(crate) fn bare_build_command(
    data_root: &Path,
    args: &BuildArgs,
    cwd: &Path,
    bun: Option<PathBuf>,
) -> Result<(std::process::Command, std::io::PipeReader)> {
    command(data_root, args, cwd, bun, None, true, None)
}

#[cfg(unix)]
fn command(
    data_root: &Path,
    args: &BuildArgs,
    cwd: &Path,
    bun: Option<PathBuf>,
    workspace: Option<&Path>,
    bare: bool,
    profile_request: Option<&Path>,
) -> Result<(std::process::Command, std::io::PipeReader)> {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    platform()?;
    validate_args(args)?;
    let resumed = args
        .resume
        .as_deref()
        .map(|id| resume_session(data_root, id))
        .transpose()?;
    let source = resumed
        .as_ref()
        .map(|session| session.repo_root.as_path())
        .unwrap_or(cwd);
    let bare = bare || resumed.as_ref().is_some_and(|session| session.bare);
    let git_root = match corral_git::repo_root(source) {
        Ok(root) => root,
        Err(_) if bare => {
            std::fs::canonicalize(source).context("resolving bare session directory")?
        }
        Err(error) => return Err(refuse("not_a_repository", format!("{error:#}"))),
    };
    let repo_root = resumed.map(|session| session.repo_root).unwrap_or(git_root);
    let workspace = if bare {
        None
    } else if args.resume.is_some() {
        workspace_for(&repo_root)?
    } else {
        workspace.map(Path::to_path_buf)
    };
    let workspace = workspace
        .as_deref()
        .map(std::fs::canonicalize)
        .transpose()
        .context("resolving the workspace root")?;
    let config = match &workspace {
        Some(root) => ConfigLayers::load(&root.join(".corral/corral.toml"))?.config()?,
        None => Config::default_for(&repo_root),
    };
    // Personal profiles are available without adopting personal build policy/assets/defaults.
    let personal = if workspace.is_none() {
        ConfigLayers::user_path()
            .filter(|path| path.is_file())
            .map(|path| Config::load(&path))
            .transpose()?
    } else {
        None
    };
    let profile_config = personal.as_ref().unwrap_or(&config);
    let problems = config.build.problems();
    if !problems.is_empty() {
        return Err(refuse("usage_error", problems.join("; ")));
    }
    for profile in config.build.profiles.values() {
        let (provider, _) = profile
            .model
            .split_once('/')
            .context("build profile model must be <provider>/<id>")?;
        require_provider(provider)?;
    }
    let project = workspace.as_ref().and_then(|root| {
        config
            .projects
            .iter()
            .filter_map(|(name, project)| {
                let path = root.join(&project.path).canonicalize().ok()?;
                repo_root
                    .starts_with(&path)
                    .then_some((name, project, path))
            })
            .max_by_key(|(_, _, path)| path.components().count())
    });
    let assets = match &config.agent.assets_dir {
        Some(dir) => dir.clone(),
        None => super::install::installed_in(data_root)?.ok_or_else(|| {
            refuse(
                "assets_unavailable",
                "no corral-pi asset installation; run `corral agent install`",
            )
        })?,
    };
    let assets =
        std::path::absolute(&assets).with_context(|| format!("resolving {}", assets.display()))?;
    let compiled = compiled_host(&assets);
    if compiled.is_none() && !assets.join(BUILD_ENTRY).is_file() {
        return Err(refuse(
            "build_unavailable",
            format!(
                "{} has no {BUILD_ENTRY}; reinstall with `corral agent install`",
                assets.display()
            ),
        ));
    }
    let compat = read_compat(&assets)?;
    let mut command = match compiled {
        Some(host) => {
            let mut command = std::process::Command::new(host);
            command.arg("build");
            command
        }
        None => {
            let bun =
                bun.ok_or_else(|| refuse("bun_unavailable", "no executable `bun` on PATH"))?;
            let version = bun_version(&bun)?;
            if !version_satisfies(&version, &compat.range) {
                return Err(refuse(
                    "bun_version_unsupported",
                    format!(
                        "bun {version} at {} is outside the GO manifest's tested range {}",
                        bun.display(),
                        compat.range
                    ),
                ));
            }
            let mut command = std::process::Command::new(bun);
            command.arg("--no-env-file").arg(assets.join(BUILD_ENTRY));
            command
        }
    };
    let (recorded, recorded_mixed) = args
        .resume
        .as_deref()
        .map(|id| profiles::recorded(data_root, id))
        .transpose()?
        .unwrap_or_default();
    let recorded_paid = recorded.as_ref().is_some_and(|route| route.paid.is_some());
    let route = if let Some(name) = &args.profile {
        profiles::resolve(profile_config, name, args)?
    } else if let Some(route) = recorded {
        profiles::validate_recorded(profile_config, &route)?;
        profiles::override_recorded(route, args)?
    } else if workspace.is_some() && !bare {
        let name = config.assistant.agent.as_ref().unwrap_or(&config.pm.agent);
        profiles::resolve(&config, name, args)?
    } else {
        let (provider, model, effort) = if bare {
            bare_model(args)?
        } else {
            model(&config, false, args)?
        };
        profiles::Route {
            name: "default".into(),
            label: "Default".into(),
            provider,
            model,
            effort,
            binding: None,
            paid: None,
        }
    };

    profiles::require_host(&assets, args.profile.is_some(), route.paid.is_some(), false)?;
    let provider = route.provider.clone();
    let model = route.model.clone();
    let effort = route.effort.clone();
    // Bare API sessions run without a ledger; spending caps are disabled.
    if route.paid.is_some() && workspace.is_none() && !bare {
        return Err(refuse(
            "paid_admission_required",
            "metered Build profiles require a corral workspace for durable spending accounting",
        ));
    }
    if let Some(binding) = &route.binding {
        if binding.connection.api_key() {
            super::connections::compatible(&assets, binding)?;
        }
        super::hooks::preflight::connection_credential(data_root, binding.connection)?;
    } else {
        super::hooks::preflight::credential(data_root, Some(require_provider(&provider)?.vendor))?;
    }
    let aws = if workspace.is_some() && provider == "amazon-bedrock" {
        super::aws_profile::read_binding(data_root)?
    } else {
        None
    };
    let snapshot = config.agent.capabilities.resolve_for(
        AgentRole::Assistant,
        project.as_ref().map(|(name, _, _)| name.as_str()),
        Some(&compat.capabilities),
    );
    let policy = if bare {
        BuildExit::Review
    } else {
        project
            .as_ref()
            .map(|(_, project, _)| project.build_exit.unwrap_or(BuildExit::Review))
            .unwrap_or(BuildExit::None)
    };
    let retained_mixed = match (&workspace, &args.resume) {
        (Some(root), Some(id)) => Store::open(root)?
            .build_paid_ledger(id)?
            .is_some_and(|ledger| ledger.mixed.is_some()),
        _ => false,
    };
    let api = mixed_api(&config);
    let mut mixed = api
        .as_deref()
        .filter(|_| workspace.is_some() && !bare && (args.resume.is_none() || retained_mixed));
    anyhow::ensure!(
        !recorded_mixed || retained_mixed,
        "mixed Build allowance is missing; resume cannot create a new balance"
    );
    let mut child_routes = std::collections::BTreeMap::new();
    let mut paid_routes = std::collections::BTreeMap::new();
    if let Some(api) = mixed {
        let admitted = (|| -> Result<()> {
            profiles::require_host(&assets, false, false, true)?;
            for (preset, name) in &api.profiles {
                let child = profiles::resolve(profile_config, name, &BuildArgs::default(), false)?;
                let binding = child
                    .binding
                    .filter(|binding| binding.connection.api_key())
                    .context(
                        "mixed Build child profile must use an explicitly approved API route",
                    )?;
                super::connections::compatible(&assets, &binding)?;
                super::hooks::preflight::connection_credential(data_root, binding.connection)?;
                paid_routes.insert(name.clone(), binding.clone());
                child_routes.insert(preset, binding);
            }
            Ok(())
        })();
        match admitted {
            // A derived default must not stop a Build that would start without it. A resumed
            // mixed Build has a ledger to honour, so it refuses like a configured approval.
            Err(error) if config.build.api.is_none() && !retained_mixed => {
                eprintln!(
                    "corral build: default API child routes are unavailable, so children keep \
                     their configured routes: {error:#}"
                );
                child_routes.clear();
                paid_routes.clear();
                mixed = None;
            }
            admitted => admitted?,
        }
    }
    anyhow::ensure!(
        !retained_mixed || mixed.is_some(),
        "mixed Build approval was revoked"
    );
    let mut child_routes = std::collections::BTreeMap::new();
    let mut paid_routes = std::collections::BTreeMap::new();
    if let Some(api) = mixed {
        for (preset, name) in &api.profiles {
            let child = profiles::resolve(profile_config, name, &BuildArgs::default())?;
            let binding = child
                .binding
                .filter(|binding| binding.connection.api_key())
                .context("mixed Build child profile must use an explicitly approved API route")?;
            super::connections::compatible(&assets, &binding)?;
            super::hooks::preflight::connection_credential(data_root, binding.connection)?;
            paid_routes.insert(name.clone(), binding.clone());
            child_routes.insert(preset, binding);
        }
        if let Some(binding) = route
            .binding
            .as_ref()
            .filter(|binding| binding.connection.api_key())
    {
        paid_routes.insert(route.name.clone(), binding.clone());
    }
    let profiles: Vec<_> = config
        .build
        .profiles
        .iter()
        .map(|(name, profile)| {
            let mut value = json!({"name": name, "model": profile.model});
            if let Some(binding) = child_routes.get(&name) {
                value["model"] = json!(format!(
                    "{}/{}",
                    binding.connection.provider(),
                    binding.model
                ));
                if let Some(effort) = &binding.effort {
                    value["effort"] = json!(effort);
                }
                value["binding"] = json!(binding);
            } else if let Some(effort) = &profile.effort {
                value["effort"] = json!(effort);
            }
            if let Some(description) = &profile.description {
                value["description"] = json!(description);
            }
            value
        })
        .collect();
    let mut message = json!({
        "version": 1, "assets": assets, "data_root": std::path::absolute(data_root)?,
        "repo_root": repo_root, "here": args.here, "provider": provider, "model": model,
        "capabilities": capabilities_payload(&snapshot, Some(&repo_root)),
        "settings": {"exit_policy": policy, "max_parallel": config.build.max_parallel, "profiles": profiles},
    });
    message["route"] = json!(route);
    if let Some(binding) = &route.binding {
        message["binding"] = json!(binding);
    }
    let catalog: Vec<_> = profile_config
        .profiles
        .keys()
        .filter_map(|name| {
            profiles::resolve(profile_config, name, &BuildArgs::default())
                .ok()
                .map(|route| json!(route))
        })
        .collect();
    message["profile_catalog"] = json!(catalog);
    if let Some(path) = profile_request {
        message["profile_request_path"] = json!(path);
    }
    if bare {
        message["bare"] = json!(true);
    } else {
        message["corral"] = json!(crate::prompt::corral_bin()?);
    }
    if let Some(root) = &workspace {
        message["workspace_root"] = json!(root);
    }
    if let Some((name, _, _)) = &project {
        message["project"] = json!(name);
    }
    if let Some(name) = &args.name {
        message["name"] = json!(name);
    }
    if let Some(resume) = &args.resume {
        message["resume"] = json!(resume);
    }
    if let Some(effort) = effort {
        message["effort"] = json!(effort);
    }
    if let Some(aws) = aws {
        message["aws"] = json!(aws);
    }
    if route.paid.is_some() && mixed.is_none() {
        message["settings"]["profiles"] = json!([]);
    }
    let mut bytes = serde_json::to_vec(&message)?;
    // Leave room for the fixed-shape paid capability before creating/fencing its ledger.
    if bytes.len() + usize::from(route.paid.is_some() || mixed.is_some()) * 192 > MESSAGE_MAX_BYTES
    {
        return Err(refuse(
            "launch_too_large",
            format!("the launch message exceeds {MESSAGE_MAX_BYTES} bytes"),
        ));
    }
    if let Some(root) = &workspace
        && (route.paid.is_some() || mixed.is_some() || args.resume.is_some())
    {
        let store = Store::open(root)?;
        if recorded_paid
            && let Some(resume) = &args.resume
            && store.build_paid_ledger(resume)?.is_none()
        {
            return Err(refuse(
                "paid_admission_required",
                "recorded Build allowance is missing; recovery cannot initialize a fresh balance",
            ));
        }
        if let Some(resume) = &args.resume
            && let Some(sticky) = crate::build_session::paid::sticky(&store, resume)?
            && (sticky != route.name || (route.paid.is_none() && mixed.is_none()))
        {
            return Err(refuse(
                "paid_admission_required",
                "paid Build route is sticky; resume it with its original profile",
            ));
        }
        let paid = if let Some(api) = mixed {
            Some(crate::build_session::paid::open_mixed(
                &store,
                &route.name,
                route.binding.as_ref(),
                &api.profiles,
                &paid_routes,
                args.resume.as_deref(),
            )?)
        } else if let Some(binding) = route.binding.as_ref().filter(|b| b.connection.api_key()) {
            Some(crate::build_session::paid::open(
                &store,
                &route.name,
                binding,
                args.resume.as_deref(),
            )?)
        } else {
            None
        };
        if let Some(paid) = paid {
            bytes.pop();
            bytes.extend_from_slice(b",\"paid\":");
            serde_json::to_writer(&mut bytes, &paid)?;
            bytes.push(b'}');
        }
    }
    let (reader, mut writer) = std::io::pipe().context("creating the launch pipe")?;
    writer
        .write_all(&bytes)
        .context("writing the launch message")?;
    drop(writer);
    rustix::io::fcntl_setfd(&reader, rustix::io::FdFlags::empty())
        .context("letting the host inherit the launch pipe")?;
    command
        .arg("--launch-fd")
        .arg(reader.as_raw_fd().to_string())
        .current_dir(&assets);
    Ok((command, reader))
}

#[cfg(not(unix))]
pub fn build_command(
    _data_root: &Path,
    _args: &BuildArgs,
    _cwd: &Path,
    _bun: Option<PathBuf>,
    _workspace: Option<&Path>,
) -> Result<(std::process::Command, std::io::PipeReader)> {
    Err(refuse(
        "unsupported_platform",
        "the corral-pi host runs on Unix",
    ))
}

/// Supervises rich Build profile handoffs without nesting host processes.
#[cfg(unix)]
pub fn build(args: &BuildArgs) -> Result<()> {
    supervisor::run(args, false)
}

#[cfg(unix)]
pub fn bare(args: &BuildArgs) -> Result<()> {
    supervisor::run(args, true)
}

#[cfg(not(unix))]
pub fn bare(_args: &BuildArgs) -> Result<()> {
    platform()
}

#[cfg(not(unix))]
pub fn build(_args: &BuildArgs) -> Result<()> {
    platform()
}

/// A build session the host recorded in `<data_root>/build/<id>/session.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSessionSummary {
    pub id: String,
    pub name: String,
    pub repo_root: PathBuf,
    pub worktree: PathBuf,
    pub here: bool,
    pub bare: bool,
    pub project: Option<String>,
    pub updated: String,
    /// The exit choice (`keep`, `ship`, `land`, `iterate`) once one is recorded.
    pub exited: Option<String>,
}

/// The largest `session.json` [`list_sessions`] reads.
const SESSION_MAX_BYTES: u64 = 64 * 1024;
/// The most recently modified session directories [`list_sessions`] reads.
const SESSION_DIRS: usize = 32;

/// The fields of the host's camelCase `BuildSessionRecord` the launcher needs.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    id: String,
    name: String,
    updated: String,
    repo_root: PathBuf,
    worktree: PathBuf,
    here: bool,
    #[serde(default)]
    bare: bool,
    project: Option<String>,
    session_file: Option<String>,
    exit: Option<SessionExit>,
}

#[derive(serde::Deserialize)]
struct SessionExit {
    choice: String,
}

/// The resumable build sessions under `data_root`, newest `updated` first. Only the
/// [`SESSION_DIRS`] most recently modified session directories are read; a record that is
/// unreadable, larger than [`SESSION_MAX_BYTES`], malformed, filed under another id or without a
/// parent transcript (`sessionFile`) is skipped.
pub fn list_sessions(data_root: &Path) -> Vec<BuildSessionSummary> {
    let Ok(entries) = std::fs::read_dir(data_root.join("build")) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            meta.is_dir().then(|| (meta.modified().ok(), entry.path()))
        })
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut sessions: Vec<_> = dirs
        .into_iter()
        .take(SESSION_DIRS)
        .filter_map(|(_, dir)| read_session(&dir))
        .collect();
    sessions.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));
    sessions.into_iter().map(|(_, session)| session).collect()
}

fn read_session(dir: &Path) -> Option<(jiff::Timestamp, BuildSessionSummary)> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(dir.join("session.json"))
        .ok()?
        .take(SESSION_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > SESSION_MAX_BYTES {
        return None;
    }
    let record: SessionRecord = serde_json::from_slice(&bytes).ok()?;
    if record.session_file.is_none() || dir.file_name() != Some(std::ffi::OsStr::new(&record.id)) {
        return None;
    }
    let updated = record.updated.parse::<jiff::Timestamp>().ok()?;
    Some((
        updated,
        BuildSessionSummary {
            id: record.id,
            name: record.name,
            repo_root: record.repo_root,
            worktree: record.worktree,
            here: record.here,
            bare: record.bare,
            project: record.project,
            updated: record.updated,
            exited: record.exit.map(|exit| exit.choice),
        },
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pi_build_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[test]
fn workspace_bedrock_launch_forwards_the_validated_binding() {
    use std::os::unix::fs::PermissionsExt;

    let mut fx = crate::agent::hooks::pm::tests::fixture();
    let data = tempfile::tempdir().unwrap();
    let assets = fx.ctx.store.config.agent.assets_dir.as_ref().unwrap();
    std::fs::create_dir_all(assets.join("src/host")).unwrap();
    std::fs::write(assets.join(BUILD_ENTRY), "export {}").unwrap();
    std::fs::write(
        assets.join("compat.json"),
        r#"{"verdict":"GO","bun":{"range":">=1.4.0 <2.0.0"}}"#,
    )
    .unwrap();
    let profile = fx.ctx.store.config.profiles.get_mut("pm-pi").unwrap();
    profile.vendor = Some("amazon-bedrock".into());
    profile.model = Some("us.anthropic.claude-sonnet-4-5-20250929-v1:0".into());
    fx.ctx.store.save_config().unwrap();
    let auth = data.path().join("auth");
    std::fs::create_dir_all(&auth).unwrap();
    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o700)).unwrap();
    let binding = auth.join("amazon-bedrock.json");
    std::fs::write(
        &binding,
        r#"{"type":"aws_profile","profile":"build-dev","region":"us-west-2"}"#,
    )
    .unwrap();
    std::fs::set_permissions(&binding, std::fs::Permissions::from_mode(0o600)).unwrap();
    let bun = data.path().join("bun");
    crate::agent::test_support::write_executable(&bun, "#!/bin/sh\necho 1.4.2\n");
    let (_, reader) = build_command(
        data.path(),
        &BuildArgs::default(),
        &fx.root().join("app"),
        Some(bun.clone()),
        Some(fx.root()),
    )
    .unwrap();
    let message: serde_json::Value = serde_json::from_reader(reader).unwrap();
    assert_eq!(message["provider"], "amazon-bedrock");
    assert_eq!(
        message["aws"],
        json!({"profile":"build-dev", "region":"us-west-2"})
    );

    std::fs::write(
        &binding,
        r#"{"type":"aws_profile","profile":"build-dev","region":"cn-north-1"}"#,
    )
    .unwrap();
    assert!(
        build_command(
            data.path(),
            &BuildArgs::default(),
            &fx.root().join("app"),
            Some(bun),
            Some(fx.root()),
        )
        .is_err()
    );
}

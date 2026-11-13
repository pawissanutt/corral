//! Untethered build launches: read configuration and credentials without opening a store, and list
//! the build sessions the host recorded so the launcher can offer them for resume.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::AgentRole;
use corral_core::config::{BuildExit, Config};
use corral_core::{ConfigLayers, ProfileChoice, Store};
use serde_json::json;

use super::bridge::assets::{bun_version, compiled_host, find_bun, read_compat, version_satisfies};
use super::hooks::pm::Overrides;
use super::launch::{capabilities_payload, host_provider};
use super::pi::{DEFAULT_PROVIDER, platform, refuse};

const BUILD_ENTRY: &str = "src/host/build.ts";
const MESSAGE_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildArgs {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
    pub here: bool,
    pub name: Option<String>,
    pub resume: Option<String>,
}

fn validate_args(args: &BuildArgs) -> Result<()> {
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
    if workspace {
        // Match assistant::profile's selection, using only its pure config resolution and overrides.
        let name = config.assistant.agent.as_ref().unwrap_or(&config.pm.agent);
        let mut resolved = config.resolve_profile(
            &ProfileChoice {
                profile: Some(name.clone()),
                ..ProfileChoice::default()
            },
            None,
            None,
        )?;
        if !super::hooks::is_corral_pi(&resolved.harness) {
            return Err(refuse(
                "assistant_profile",
                format!("profile '{name}' is not corral-pi"),
            ));
        }
        super::hooks::pm::apply(
            &mut resolved,
            &Overrides {
                model: args.model.clone(),
                provider: args.provider.clone(),
                effort: args.effort.clone(),
                ..Overrides::default()
            },
        )
        .map_err(|error| refuse("usage_error", format!("{error:#}")))?;
        let model = resolved.model.ok_or_else(|| {
            refuse(
                "assistant_profile",
                format!("profile '{name}' names no model"),
            )
        })?;
        let provider = host_provider(resolved.vendor.as_deref(), &model).ok_or_else(|| {
            refuse(
                "assistant_profile",
                format!("profile '{name}' has no known provider"),
            )
        })?;
        require_provider(&provider)?;
        return Ok((provider, model, resolved.effort));
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
    let git_root = corral_git::repo_root(source)
        .map_err(|error| refuse("not_a_repository", format!("{error:#}")))?;
    let repo_root = resumed.map(|session| session.repo_root).unwrap_or(git_root);
    let workspace = if args.resume.is_some() {
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
    let (provider, model, effort) = model(&config, workspace.is_some(), args)?;
    super::hooks::preflight::credential(data_root, Some(require_provider(&provider)?.vendor))?;
    let snapshot = config.agent.capabilities.resolve_for(
        AgentRole::Assistant,
        project.as_ref().map(|(name, _, _)| name.as_str()),
        Some(&compat.capabilities),
    );
    let policy = project
        .as_ref()
        .map(|(_, project, _)| project.build_exit.unwrap_or(BuildExit::Review))
        .unwrap_or(BuildExit::None);
    let profiles: Vec<_> = config
        .build
        .profiles
        .iter()
        .map(|(name, profile)| {
            let mut value = json!({"name": name, "model": profile.model});
            if let Some(effort) = &profile.effort {
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
        "corral": crate::prompt::corral_bin()?,
        "repo_root": repo_root, "here": args.here, "provider": provider, "model": model,
        "capabilities": capabilities_payload(&snapshot, Some(&repo_root)),
        "settings": {"exit_policy": policy, "max_parallel": config.build.max_parallel, "profiles": profiles},
    });
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
    let bytes = serde_json::to_vec(&message)?;
    if bytes.len() > MESSAGE_MAX_BYTES {
        return Err(refuse(
            "launch_too_large",
            format!("the launch message exceeds {MESSAGE_MAX_BYTES} bytes"),
        ));
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

/// Execs the build host, keeping its launch descriptor alive through exec.
#[cfg(unix)]
pub fn build(args: &BuildArgs) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let workspace = if args.resume.is_some() {
        None
    } else {
        workspace_for(&cwd)?
    };
    let (mut command, _reader) = build_command(
        &super::install::data_root()?,
        args,
        &cwd,
        find_bun(),
        workspace.as_deref(),
    )?;
    Err(command.exec()).context("executing the build Pi host")
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
            project: record.project,
            updated: record.updated,
            exited: record.exit.map(|exit| exit.choice),
        },
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pi_build_tests.rs"]
mod tests;

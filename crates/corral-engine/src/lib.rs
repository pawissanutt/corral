//! Small, locally trusted task engine. All persistent operations share SQLite
//! transactions with their events; runners are replaceable at the boundary.
//!
//! Submit operations through [`Engine::apply`]. The daemon and CLI can build read
//! models via [`Store`]; neither needs direct SQL access.

pub mod artifacts;
pub mod board_plan;
mod checkout;
mod citations;
pub mod config;
mod dispatch;
pub mod error;
pub mod events;
pub mod extension;
pub mod facts;
mod land;
mod lens_tasks;
pub mod model;
#[cfg(unix)]
pub mod pi_hosted_tools;
#[cfg(unix)]
mod pi_preferences;
#[cfg(unix)]
pub mod pi_startup;
mod pipeline;
mod prompt;
pub mod react;
mod review;
pub mod runner;
pub mod store;
mod sync;
pub mod tasks;
pub mod usage;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub use citations::ArtifactCitation;
pub use config::Config;
pub use error::{as_refusal, refuse};
pub use model::*;
pub use runner::{LaunchSpec, Runner};
pub use store::Store;

pub mod testing {
    pub use crate::runner::FakeRunner;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PipelineMode {
    #[default]
    Spawn,
    Inline,
    Off,
}

pub struct Engine {
    pub root: PathBuf,
    pub state_dir: PathBuf,
    pub config: Config,
    pub store: Store,
    pub runner: Box<dyn Runner>,
    pub actor: Actor,
    pub pipeline: PipelineMode,
}

impl Engine {
    pub fn apply(&self, op: Op, force: bool) -> Result<Applied> {
        if force
            && !matches!(
                op,
                Op::Move { .. } | Op::Dispatch { .. } | Op::Approve { .. }
            )
        {
            return Err(refuse(
                "this operation does not accept --force",
                vec![Next::run(op)],
            ));
        }
        let mut applied = match op {
            Op::New { task } => applied_task(self.new_task(task)?),
            Op::LensDraft {
                project,
                kind,
                about,
                ready,
            } => applied_task(self.draft_lens(&project, &kind, &about, ready)?),
            Op::LensRepair { project, id, ready } => {
                applied_task(self.repair_lens(&project, &id, ready)?)
            }
            Op::Edit { id, edit } => applied_task(self.edit_task(&id, edit)?),
            Op::Comment { id, text } => applied_task(self.comment(&id, &text)?),
            Op::Move { id, status } => applied_task(self.move_task(&id, status, force)?),
            Op::Dispatch { id, profile, base } => {
                let run = self.dispatch(&id, profile.as_deref(), base.as_deref(), force)?;
                self.applied_for(&id, Some(run))?
            }
            Op::Reply { id, text } => self.applied_for(&id, Some(self.reply(&id, &text)?))?,
            Op::Stop { id } => self.applied_for(&id, Some(self.stop(&id)?))?,
            Op::Retry {
                id,
                profile,
                fresh,
                note,
            } => {
                let run = self.retry(&id, profile.as_deref(), fresh, note.as_deref())?;
                self.applied_for(&id, Some(run))?
            }
            Op::Done { id, summary } => {
                let run = self.done(&id, &summary)?;
                let run = run
                    .map(|run| self.store.get_run(&id, &run.id))
                    .transpose()?
                    .flatten();
                self.applied_for(&id, run)?
            }
            Op::Gate { id } => {
                let check = self.gate(&id)?;
                let mut applied = self.applied_for(&id, None)?;
                applied.artifact = check
                    .log
                    .as_deref()
                    .map(|log| self.store.get_artifact(log))
                    .transpose()?
                    .flatten();
                applied
            }
            Op::Review { id, profile } => {
                let run = self.review(&id, profile.as_deref())?;
                self.applied_for(&id, Some(run))?
            }
            Op::Verdict { id, approve, notes } => {
                self.applied_for(&id, self.verdict(&id, approve, notes.as_deref())?)?
            }
            Op::Changes { id, text } => self.applied_for(&id, self.changes(&id, &text)?)?,
            Op::Approve { id, onto } => {
                let run = self.approve(&id, onto.as_deref(), force)?;
                self.applied_for(&id, self.store.get_run(&id, &run.id)?)?
            }
            Op::Drop { id } => applied_task(self.drop_task(&id)?),
            Op::Reopen { id } => applied_task(self.reopen(&id)?),
            Op::ArtifactAdd {
                id,
                path,
                kind,
                title,
            } => {
                let mut applied = self.applied_for(&id, None)?;
                applied.artifact = Some(self.add_artifact(&id, &path, kind, title.as_deref())?);
                applied
            }
            Op::UsageAdd { usage } => {
                self.report_usage(usage)?;
                Applied::default()
            }
            Op::Sync => {
                self.sync()?;
                Applied::default()
            }
        };
        applied.seq = self.store.latest_seq()?;
        Ok(applied)
    }

    fn applied_for(&self, id: &str, run: Option<Run>) -> Result<Applied> {
        Ok(Applied {
            task: Some(self.store.require_task(id)?),
            run,
            ..Applied::default()
        })
    }

    pub fn init(root: impl AsRef<Path>, first_id: Option<u32>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("resolve workspace root for initialization")?;
        let state_dir = root.join(".corral-next");
        if state_dir.exists() {
            return Err(refuse(
                "workspace already has .corral-next; use corral list --all",
                vec![],
            ));
        }
        fs::create_dir(&state_dir).context("create .corral-next state directory")?;
        for directory in ["blobs/sha256", "runs", "locks"] {
            fs::create_dir_all(state_dir.join(directory))
                .with_context(|| format!("create state directory {directory}"))?;
        }
        let config = Config::default();
        fs::write(state_dir.join("corral.toml"), include_str!("starter.toml"))
            .context("write initial configuration")?;
        let store = Store::open(&state_dir.join("corral.db"))?;
        if let Some(first_id) = first_id {
            store.set_first_id(first_id)?;
        }
        let runner = select_runner(&config)?;
        Ok(Self {
            root,
            state_dir,
            config,
            store,
            runner,
            actor: actor_from_env(),
            pipeline: PipelineMode::Spawn,
        })
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(root.as_ref(), None)
    }

    pub fn open_with_runner(root: impl AsRef<Path>, runner: Box<dyn Runner>) -> Result<Self> {
        Self::open_with(root.as_ref(), Some(runner))
    }

    /// Route socket-backed sessions through Pi, and ordinary sessions through
    /// the configured placement runner. Use `runner` directly for non-Pi starts.
    pub fn session_runner(&self) -> runner::pi::PiRunner<'_> {
        runner::pi::PiRunner::new(self.runner.as_ref())
    }

    fn open_with(root: &Path, runner: Option<Box<dyn Runner>>) -> Result<Self> {
        let root = root.canonicalize().context("resolve workspace root")?;
        let state_dir = root.join(".corral-next");
        if !state_dir.join("corral.db").is_file() {
            return Err(refuse(
                "workspace is not initialized; run corral init",
                vec![],
            ));
        }
        let config = Config::load(&state_dir.join("corral.toml"))?;
        let store = Store::open(&state_dir.join("corral.db"))?;
        let runner = match runner {
            Some(runner) => runner,
            None => select_runner(&config)?,
        };
        Ok(Self {
            root,
            state_dir,
            config,
            store,
            runner,
            actor: actor_from_env(),
            pipeline: PipelineMode::Spawn,
        })
    }

    pub fn discover_root(start_dir: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
        discover_root(start_dir, explicit)
    }
}

fn applied_task(task: Task) -> Applied {
    Applied {
        task: Some(task),
        ..Applied::default()
    }
}

fn select_runner(config: &Config) -> Result<Box<dyn Runner>> {
    match config.runner.mode.as_str() {
        "herdr" => {
            let session = config
                .runner
                .herdr_session
                .as_ref()
                .filter(|name| !name.is_empty())
                .cloned();
            Ok(Box::new(runner::HerdrRunner::new(
                corral_herdr::Herdr::new(session),
            )))
        }
        "process" => Ok(Box::new(runner::ProcessRunner)),
        mode => Err(refuse(
            format!(
                "unknown runner {mode:?}; set runner.mode to herdr or process in .corral-next/corral.toml"
            ),
            vec![],
        )),
    }
}

pub(crate) fn binary_path() -> Result<PathBuf> {
    match std::env::var_os("CORRAL_NEXT_BIN").filter(|name| !name.is_empty()) {
        Some(bin) => std::path::absolute(bin).context("resolve CORRAL_NEXT_BIN"),
        None => std::env::current_exe().context("resolve current corral executable"),
    }
}

pub fn discover_root(start_dir: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    let selected = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CORRAL_NEXT_ROOT").map(PathBuf::from));
    if let Some(root) = selected {
        let root = if root.is_absolute() {
            root
        } else {
            start_dir.join(root)
        };
        return root
            .canonicalize()
            .context("resolve explicit workspace root");
    }
    let start = start_dir
        .canonicalize()
        .context("resolve discovery directory")?;
    start
        .ancestors()
        .find(|ancestor| ancestor.join(".corral-next").is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| refuse("no ancestor contains .corral-next; run corral init", vec![]))
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;

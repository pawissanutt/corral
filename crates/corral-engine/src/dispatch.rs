use std::{fs, path::PathBuf};

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde_json::json;

use crate::{
    Actor, Blocked, Engine, LaunchSpec, Next, Op, Origin, Outcome, Role, Run, Runner, SessionRef,
    Status, Store, Task, TaskEdit, as_refusal,
    checkout::Checkout,
    config::{CommandContext, EffectiveProject, render_command, render_wrap, valid_env_key},
    events::EventKind,
    prompt::write_prompt,
    refuse,
    runner::{
        InputTag,
        pi::{PiRunner, SubmitReply},
    },
    tasks::{next_force_approve, next_retry, next_review, set_lifecycle},
};

pub(crate) struct LaunchRequest<'a> {
    pub task: &'a Task,
    pub checkout: &'a Checkout,
    pub role: Role,
    pub profile: &'a str,
    pub note: Option<&'a str>,
    pub project: Option<&'a EffectiveProject>,
    pub fresh: bool,
}

#[derive(Default)]
pub(crate) struct PendingLaunch {
    checkout: Option<(Checkout, bool)>,
    dir: Option<PathBuf>,
    session: Option<SessionRef>,
}

impl Engine {
    pub fn dispatch(
        &self,
        id: &str,
        profile: Option<&str>,
        base: Option<&str>,
        force: bool,
    ) -> Result<Run> {
        self.dispatch_as(id, profile, base, force, self.actor)
    }

    pub(crate) fn dispatch_as(
        &self,
        id: &str,
        profile: Option<&str>,
        base: Option<&str>,
        force: bool,
        actor: Actor,
    ) -> Result<Run> {
        let op = |id: &str| Op::Dispatch {
            id: id.into(),
            profile: profile.map(str::to_owned),
            base: base.map(str::to_owned),
        };
        self.check_force(force, || op(id))?;
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if !matches!(task.status, Status::Backlog | Status::Ready) {
                let move_op = Op::Move {
                    id: id.into(),
                    status: Status::Ready,
                };
                let next = if task.status == Status::Idea {
                    Next::run(move_op)
                } else {
                    Next::force(move_op)
                };
                return Err(refuse(
                    "dispatch requires a Backlog or Ready task, even with --force",
                    vec![next],
                ));
            }
            let project_name = self.dispatch_project(&task)?;
            let project = self.config.effective_project(project_name)?;
            let mut skipped = Vec::new();
            let mut next = Vec::new();
            for dependency in &task.depends_on {
                let dependency_task = store.get_task(dependency)?;
                if dependency_task
                    .as_ref()
                    .is_some_and(|dep| dep.status == Status::Done)
                {
                    continue;
                }
                if skipped.is_empty() {
                    skipped.push("dependencies");
                }
                if !force && dependency_task.is_some() {
                    next.push(Next::force(Op::Move {
                        id: dependency.clone(),
                        status: Status::Done,
                    }));
                }
            }
            let global_full = store.occupied_slots(None)? >= self.config.limits.running;
            let project_full = if let Some(limit) = project.running {
                store.occupied_slots(Some(project_name))? >= limit
            } else {
                false
            };
            if global_full {
                skipped.push("limits.running");
            }
            if project_full {
                skipped.push("project.running");
            }
            if !skipped.is_empty() && !force {
                next.insert(0, Next::force(op(id)));
                return Err(refuse(
                    format!("dispatch blocked by {}", skipped.join(", ")),
                    next,
                ));
            }
            let profile = self
                .config
                .resolve_profile(&task, profile)
                .map_err(|error| {
                    self.profile_choices(
                        error,
                        |profile| Op::Dispatch {
                            id: id.into(),
                            profile: Some(profile),
                            base: base.map(str::to_owned),
                        },
                        force,
                    )
                })?;
            let latest = store.latest_run(id, Role::Worker)?;
            let repo = self
                .root
                .join(&project.path)
                .canonicalize()
                .context("resolve project repository path")?;
            let branch = format!("{}{id}", project.branch_prefix);
            let base = resolve_base(
                &repo,
                &branch,
                base.or(task.base.as_deref()).or(project.base.as_deref()),
                |base| Op::Dispatch {
                    id: id.into(),
                    profile: Some(profile.clone()),
                    base: Some(base),
                },
                force,
            )?;
            let mut checkout = Checkout {
                path: repo.join(".worktrees").join(id),
                repo,
                branch,
                base,
                base_sha: String::new(),
            };
            let reuse = latest.as_ref().is_some_and(|last| {
                last.branch == checkout.branch && last.worktree == checkout.path
            });
            self.prepare_checkout(&mut checkout, pending, reuse, false, None)?;
            let run = self.start_run(
                store,
                pending,
                LaunchRequest {
                    task: &task,
                    checkout: &checkout,
                    role: Role::Worker,
                    profile: &profile,
                    note: None,
                    project: Some(&project),
                    fresh: false,
                },
            )?;
            set_lifecycle(&mut task, Status::Running);
            if actor == Actor::Owner {
                task.round = 0;
            }
            store.update_task(&task)?;
            self.event(
                store,
                Some(id),
                Some(&run.id),
                EventKind::Dispatched,
                actor,
                json!({"profile":run.profile,"base":run.base}),
            )?;
            if force {
                self.event(
                    store,
                    Some(id),
                    Some(&run.id),
                    EventKind::Forced,
                    actor,
                    json!({"operation":"dispatch","skipped":skipped}),
                )?;
            }
            self.supersede_runs(store, id, |previous| previous.id != run.id)?;
            Ok(run)
        })
    }

    fn dispatch_project<'a>(&self, task: &'a Task) -> Result<&'a str> {
        task.project.as_deref().filter(|name| self.config.projects.contains_key(*name)).ok_or_else(|| {
            let next = self.config.projects.keys().map(|project| {
                Next::run(Op::Edit {
                    id: task.id.clone(),
                    edit: TaskEdit { project: Some(Some(project.clone())), ..TaskEdit::default() },
                })
            }).collect();
            refuse("dispatch requires a configured project; set the task's project or update .corral-next/corral.toml", next)
        })
    }

    pub fn reply(&self, id: &str, text: &str) -> Result<Run> {
        self.reply_with(id, None, text, |runner, session, origin| {
            runner.send(session, text, origin)
        })
        .map(|(run, ())| run)
    }

    pub fn reply_tagged(
        &self,
        id: &str,
        run: &str,
        text: &str,
        tag: InputTag<'_>,
    ) -> Result<SubmitReply> {
        self.reply_with(id, Some(run), text, |runner, session, origin| {
            runner.send_tagged(session, text, origin, tag)
        })
        .map(|(_, reply)| reply)
    }

    fn reply_with<T>(
        &self,
        id: &str,
        expected_run: Option<&str>,
        text: &str,
        send: impl FnOnce(PiRunner<'_>, &SessionRef, Origin) -> Result<T>,
    ) -> Result<(Run, T)> {
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            let mut run = self.last_worker(store, &task)?;
            if expected_run.is_some_and(|expected| expected != run.id) {
                return Err(refuse("reply requires the current worker run", vec![]));
            }
            if matches!(task.status, Status::Done | Status::Dropped) {
                return Err(refuse("reply requires a live worker", vec![next_retry(id)]));
            }
            let Some(session) = self.live_session(&run)? else {
                return Err(refuse("reply requires a live worker", vec![next_retry(id)]));
            };
            let origin = if self.actor == Actor::Auto {
                Origin::Auto
            } else {
                Origin::Owner
            };
            let delivered = send(self.session_runner(), session, origin)
                .map_err(|error| {
                    if let Some(refusal) = as_refusal(&error) {
                        refuse(
                            &refusal.reason,
                            vec![Next::run(Op::Retry {
                                id: id.into(),
                                profile: None,
                                fresh: false,
                                note: Some(text.into()),
                            })],
                        )
                    } else {
                        error
                    }
                })
                .context("deliver worker reply")?;
            if run.quiet_since.take().is_some() {
                store.update_run(&run)?;
            }
            self.event(
                store,
                Some(id),
                Some(&run.id),
                EventKind::Replied,
                self.actor,
                json!({"text":text}),
            )?;
            if task.status == Status::Blocked {
                set_lifecycle(&mut task, Status::Running);
                if self.actor == Actor::Owner {
                    task.round = 0;
                }
                store.update_task(&task)?;
                self.event(
                    store,
                    Some(id),
                    Some(&run.id),
                    EventKind::Resumed,
                    self.actor,
                    json!({}),
                )?;
            }
            Ok((run, delivered))
        })
    }

    pub fn stop(&self, id: &str) -> Result<Run> {
        self.stop_selected(id, None)
    }

    /// Stop the addressed live run, not another role's run on the same task.
    pub fn stop_run(&self, id: &str, run: &str) -> Result<Run> {
        self.stop_selected(id, Some(run))
    }

    fn stop_selected(&self, id: &str, selected: Option<&str>) -> Result<Run> {
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            let runs = store.live_runs_for_task(id)?;
            for mut run in runs.into_iter().rev() {
                if selected.is_some_and(|selected| selected != run.id)
                    || self.live_session(&run)?.is_none()
                {
                    continue;
                }
                self.end_run(store, &mut run, Outcome::Stopped, true)?;
                let reason = "stopped by owner";
                self.event(
                    store,
                    Some(id),
                    Some(&run.id),
                    EventKind::Stopped,
                    self.actor,
                    json!({"reason":reason}),
                )?;
                let next = if run.role == Role::Reviewer {
                    next_review(id)
                } else {
                    next_retry(id)
                };
                self.block_in(
                    store,
                    &mut task,
                    Some(&run.id),
                    Blocked {
                        reason: reason.into(),
                        detail: None,
                        next: vec![next, next_force_approve(id)],
                    },
                    self.actor,
                )?;
                return Ok(run);
            }
            let next = if store.latest_run(id, Role::Worker)?.is_some() {
                next_retry(id)
            } else {
                dispatch_next(&task)
            };
            Err(refuse("stop requires a live session", vec![next]))
        })
    }

    pub fn retry(
        &self,
        id: &str,
        profile: Option<&str>,
        fresh: bool,
        note: Option<&str>,
    ) -> Result<Run> {
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            let last = self.last_worker(store, &task)?;
            let profile = self
                .config
                .require_profile(profile.unwrap_or(&last.profile))
                .map_err(|error| {
                    self.profile_choices(
                        error,
                        |profile| Op::Retry {
                            id: id.into(),
                            profile: Some(profile),
                            fresh,
                            note: note.map(str::to_owned),
                        },
                        false,
                    )
                })?;
            let project = self.project_for(&task)?;
            let mut checkout = Checkout::from_run(&last)?;
            self.supersede_runs(store, id, |run| run.role == Role::Worker)?;
            self.prepare_checkout(
                &mut checkout,
                pending,
                true,
                fresh,
                last.head_sha.as_deref(),
            )?;
            let run = self.start_run(
                store,
                pending,
                LaunchRequest {
                    task: &task,
                    checkout: &checkout,
                    role: Role::Worker,
                    profile: &profile,
                    note,
                    project: project.as_ref(),
                    fresh,
                },
            )?;
            set_lifecycle(&mut task, Status::Running);
            task.round = 0;
            store.update_task(&task)?;
            self.event(
                store,
                Some(id),
                Some(&run.id),
                EventKind::Resumed,
                self.actor,
                json!({"retry":true,"fresh":fresh,"note":note}),
            )?;
            Ok(run)
        })
    }

    fn prepare_checkout(
        &self,
        checkout: &mut Checkout,
        pending: &mut PendingLaunch,
        reuse: bool,
        fresh: bool,
        old_head: Option<&str>,
    ) -> Result<()> {
        corral_git::validate_branch_name(&checkout.branch).context("validate task branch name")?;
        corral_git::validate_branch_name(&checkout.base).context("validate base branch name")?;
        ensure!(
            checkout.branch != checkout.base,
            "task branch and base must differ"
        );
        let base_ref = checkout.base_ref();
        let base_sha =
            corral_git::rev_sha(&checkout.repo, &base_ref).context("resolve local base branch")?;
        let exists = checkout.branch_exists()?;
        let path_exists = checkout
            .path
            .try_exists()
            .context("inspect task worktree path")?;
        ensure!(
            reuse || !(exists || path_exists),
            "task worktree or branch already exists outside its run history"
        );
        if path_exists {
            ensure!(exists, "task worktree exists but its branch is missing");
        } else {
            let start = if fresh {
                &base_sha
            } else {
                old_head.unwrap_or(&base_sha)
            };
            corral_git::worktree_add(&checkout.repo, &checkout.path, &checkout.branch, start)
                .context("create task worktree")?;
            pending.checkout = Some((
                Checkout {
                    repo: checkout.repo.clone(),
                    path: checkout.path.clone(),
                    branch: checkout.branch.clone(),
                    base: checkout.base.clone(),
                    base_sha: String::new(),
                },
                !exists,
            ));
        }
        checkout.verify_branch()?;
        if fresh && exists {
            checkout.reset_to_base()?;
        }
        checkout.base_sha = if fresh || (!exists && old_head.is_none()) {
            base_sha
        } else {
            checkout.merge_base()?
        };
        Ok(())
    }

    pub(crate) fn start_run(
        &self,
        store: &Store,
        pending: &mut PendingLaunch,
        request: LaunchRequest<'_>,
    ) -> Result<Run> {
        let LaunchRequest {
            task,
            checkout,
            role,
            profile,
            note,
            project,
            fresh,
        } = request;
        let definition = self
            .config
            .profiles
            .get(profile)
            .with_context(|| format!("unknown profile {profile:?}"))?;
        let harness = self
            .config
            .harnesses
            .get(&definition.harness)
            .with_context(|| format!("unknown harness {:?}", definition.harness))?;
        let mut run = Run {
            id: store.next_run_id(&task.id)?,
            task: task.id.clone(),
            role,
            profile: profile.to_owned(),
            harness: definition.harness.clone(),
            model: definition.model.clone(),
            effort: definition.effort.clone(),
            worktree: checkout.path.clone(),
            branch: checkout.branch.clone(),
            base: checkout.base.clone(),
            base_sha: checkout.base_sha.clone(),
            session: None,
            started: Timestamp::now(),
            ended: None,
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        };
        let parent = self.state_dir.join("runs").join(&task.id);
        fs::create_dir_all(&parent).context("create task run directory")?;
        let dir = parent.join(&run.id);
        let pi = definition.harness == "pi";
        if pi {
            crate::runner::pi::private_dir(&dir, false)?;
        } else {
            fs::create_dir(&dir)
                .with_context(|| format!("create run directory {}", dir.display()))?;
        }
        pending.dir = Some(dir.clone());
        let prompt_file = dir.join("prompt.md");
        let bin = crate::binary_path()?;
        write_prompt(&prompt_file, &bin, task, &run, note)?;
        let label = format!("{} {}", task.id, task.title);
        let command = render_command(
            harness,
            definition,
            &CommandContext {
                prompt_file: &prompt_file,
                name: &label,
                task: &task.id,
                worktree: &run.worktree,
                corral: &bin,
            },
        )
        .context("render session launch command")?;
        let root = self.root.to_str().context("workspace root is not UTF-8")?;
        let exports = [
            ("CORRAL_NEXT_ROOT", root),
            ("CORRAL_TASK", task.id.as_str()),
            ("CORRAL_RUN", run.id.as_str()),
            (
                "CORRAL_ACTOR",
                if role == Role::Reviewer {
                    "reviewer"
                } else {
                    "agent"
                },
            ),
        ];
        let mut env = Vec::with_capacity(
            exports.len() + usize::from(pi) + project.map_or(0, |p| p.env.len()),
        );
        if let Some(project) = project {
            for (key, value) in &project.env {
                if exports.iter().any(|(reserved, _)| reserved == key)
                    || (pi && key == "CORRAL_FRESH")
                {
                    continue;
                }
                ensure!(
                    valid_env_key(key) && !value.contains('\0'),
                    "invalid project environment variable {key:?}"
                );
                env.push((key.clone(), value.clone()));
            }
        }
        if pi {
            // Only the engine decides whether the new host inherits its transcript.
            env.push(("CORRAL_FRESH".into(), if fresh { "1" } else { "0" }.into()));
        }
        env.extend(exports.map(|(key, value)| (key.to_owned(), value.to_owned())));
        let mut spec = LaunchSpec {
            label,
            cwd: run.worktree.clone(),
            command,
            env,
            dir,
            tokens: vec![
                ("task".into(), task.id.clone()),
                ("run".into(), run.id.clone()),
            ],
        };
        let wrap = project.map_or(self.config.runner.wrap.as_deref(), |p| p.wrap.as_deref());
        let prepare = |launch: &mut LaunchSpec| -> Result<()> {
            if let Some(template) = wrap {
                launch.command = render_wrap(
                    template,
                    &launch.command,
                    &self.root,
                    &launch.cwd,
                    &launch.dir,
                )
                .context("render session wrap command")?;
            }
            Ok(())
        };
        let session = if pi {
            self.session_runner().start_with(&spec, prepare)
        } else {
            prepare(&mut spec)?;
            self.runner.start(&spec)
        }
        .context("start agent session")?;
        pending.session = Some(session.clone());
        run.session = Some(session);
        store.insert_run(&run)?;
        Ok(run)
    }

    pub(crate) fn launch_transaction<T>(
        &self,
        execute: impl FnOnce(&Store, &mut PendingLaunch) -> Result<T>,
    ) -> Result<T> {
        let mut pending = PendingLaunch::default();
        self.store
            .transaction(|store| {
                execute(store, &mut pending).map_err(|error| pending.rollback(self, error))
            })
            .map_err(|error| pending.rollback(self, error))
    }

    pub(crate) fn last_worker(&self, store: &Store, task: &Task) -> Result<Run> {
        store
            .latest_run(&task.id, Role::Worker)?
            .ok_or_else(|| refuse("task has not had a worker run", vec![dispatch_next(task)]))
    }

    pub(crate) fn project_for(&self, task: &Task) -> Result<Option<EffectiveProject>> {
        task.project
            .as_deref()
            .filter(|name| self.config.projects.contains_key(*name))
            .map(|name| self.config.effective_project(name))
            .transpose()
    }

    pub(crate) fn live_session<'a>(&self, run: &'a Run) -> Result<Option<&'a SessionRef>> {
        if run.ended.is_none()
            && let Some(session) = &run.session
            && self
                .session_runner()
                .is_live(session)
                .with_context(|| format!("read session {}:{}", run.task, run.id))?
        {
            return Ok(Some(session));
        }
        Ok(None)
    }

    fn supersede_runs(
        &self,
        store: &Store,
        id: &str,
        should_stop: impl Fn(&Run) -> bool,
    ) -> Result<()> {
        for mut run in store.live_runs_for_task(id)? {
            if should_stop(&run) {
                self.end_run(store, &mut run, Outcome::Superseded, true)?;
            }
        }
        Ok(())
    }

    pub(crate) fn profile_choices(
        &self,
        error: anyhow::Error,
        op: impl Fn(String) -> Op,
        force: bool,
    ) -> anyhow::Error {
        if let Some(error) = as_refusal(&error) {
            return refuse(
                &error.reason,
                self.config
                    .profiles
                    .keys()
                    .map(|name| {
                        let op = op(name.clone());
                        if force {
                            Next::force(op)
                        } else {
                            Next::run(op)
                        }
                    })
                    .collect(),
            );
        }
        error
    }
}

impl PendingLaunch {
    fn rollback(&mut self, engine: &Engine, mut error: anyhow::Error) -> anyhow::Error {
        if let Some(session) = self.session.take()
            && let Err(cleanup) = engine.session_runner().stop(&session)
        {
            error = error.context(format!("failed to close uncommitted launch: {cleanup:#}"));
        }
        if let Some(dir) = self.dir.take() {
            if let Err(cleanup) = fs::remove_dir_all(&dir) {
                error = error.context(format!(
                    "failed to remove uncommitted run directory {}: {cleanup}",
                    dir.display()
                ));
            }
            if let Some(parent) = dir.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
        if let Some((checkout, new_branch)) = self.checkout.take() {
            if let Err(cleanup) = corral_git::worktree_remove(&checkout.repo, &checkout.path, true)
            {
                error = error.context(format!("failed to roll back task worktree: {cleanup:#}"));
            }
            if new_branch && let Err(cleanup) = checkout.delete_branch() {
                error = error.context(format!("failed to roll back task branch: {cleanup:#}"));
            }
        }
        error
    }
}

fn dispatch_next(task: &Task) -> Next {
    let id = task.id.clone();
    if matches!(task.status, Status::Backlog | Status::Ready) {
        Next::run(Op::Dispatch {
            id,
            profile: None,
            base: None,
        })
    } else {
        let op = Op::Move {
            id,
            status: Status::Backlog,
        };
        if task.status == Status::Idea {
            Next::run(op)
        } else {
            Next::force(op)
        }
    }
}

fn resolve_base(
    repo: &std::path::Path,
    task_branch: &str,
    configured: Option<&str>,
    retry: impl Fn(String) -> Op,
    force: bool,
) -> Result<String> {
    let selected = match configured {
        Some(branch) => Some(branch.to_owned()),
        None => corral_git::default_branch(repo).context("discover repository default branch")?,
    };
    let reason = match selected {
        Some(branch) if branch == task_branch => {
            format!("task branch {branch:?} cannot be its own base; choose a branch with --base")
        }
        Some(branch) => {
            if corral_git::local_branch_exists(repo, &branch)
                .context("check dispatch base branch")?
            {
                return Ok(branch);
            }
            format!("base branch {branch:?} does not exist; choose a local branch with --base")
        }
        None => "project checkout has a detached HEAD; choose a branch with --base".into(),
    };
    let next = corral_git::local_branches(repo)?
        .into_iter()
        .filter(|branch| branch != task_branch)
        .map(|branch| Next {
            op: retry(branch),
            force,
        })
        .collect();
    Err(refuse(reason, next))
}

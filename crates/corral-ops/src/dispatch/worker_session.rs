//! Reopen the worker conversation, not an empty process behind the old pane.
use super::*;
use corral_core::{RoleSession, SessionAction, SessionPolicy};

pub(super) struct Continuation {
    pub session: RoleSession,
    pub restart: bool,
}

pub(super) struct Prepared {
    pub herdr: corral_core::HerdrRef,
    command: String,
    old_pane: Option<String>,
    receipt: std::path::PathBuf,
    ctx: Ctx,
    submitted: std::cell::Cell<bool>,
}

impl Prepared {
    pub fn submit(&self, ctx: &Ctx) -> Result<()> {
        if let Some(old) = &self.old_pane
            && ctx.herdr.pane_get(old)?.is_some()
        {
            ctx.herdr.pane_close(old)?;
        }
        let command = crate::launch_exit::arm(&self.command, &self.receipt)?;
        self.submitted.set(true);
        ctx.herdr.pane_run(&self.herdr.pane_id, &command)
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.submitted.get()
            && let Err(error) = self.ctx.herdr.pane_close(&self.herdr.pane_id)
        {
            crate::messages::report(
                None,
                "session",
                format!("closing unsubmitted worker pane: {error:#}"),
            );
        }
    }
}

impl Continuation {
    pub fn prepare(ctx: &Ctx, task: &Task, run: &Run) -> Result<Self> {
        let fresh = crate::role_session::fresh(ctx, task, run, false)?;
        let pane = run.herdr.as_ref().map(|h| h.pane_id.as_str());
        let missing = match pane {
            Some(pane) => ctx.herdr.pane_get(pane)?.is_none(),
            None => true,
        };
        let exited =
            crate::launch_exit::exited(&crate::launch_exit::receipt(ctx, &task.id, &run.id));
        let restart = missing || exited || fresh.policy == SessionPolicy::Fresh;
        let session = if fresh.policy == SessionPolicy::Fresh {
            fresh
        } else {
            match crate::role_session::capture(ctx, task, run, false) {
                Ok(mut session) => {
                    session.policy = fresh.policy;
                    session.action = if restart { SessionAction::Resumed } else { SessionAction::Reused };
                    session
                }
                // A live legacy harness still has its context. Do not invent a durable ID.
                Err(error) if !restart => {
                    let mut session = run.session.clone().unwrap_or(fresh);
                    session.diagnostic = Some(format!(
                        "Worker pane retained; durable session identity unavailable: {error:#}"
                    ));
                    session
                }
                Err(error) => return Err(crate::role_session::SessionUnavailable(format!(
                    "worker context cannot be resumed; restore its recorded transcript or explicitly configure worker_session = 'fresh': {error:#}"
                )).into()),
            }
        };
        Ok(Self { session, restart })
    }

    /// Allocate outside board/task locks; replace the old process only after final admission.
    pub fn prepare_legacy(
        &self,
        ctx: &Ctx,
        task: &Task,
        text: &str,
        notice: Option<&str>,
    ) -> Result<Option<Prepared>> {
        let run = task.current_run().context("worker run disappeared")?;
        if !self.restart || run.delivery != corral_core::inbox::DeliveryMode::Legacy {
            return Ok(None);
        }
        crate::session_scope::check_launch(ctx)?;
        if let Some(reason) = crate::queue::blocked_reason(ctx, &ctx.store, task, false)? {
            bail!("cannot restart worker: {reason}");
        }
        let mut selected = run.clone();
        selected.session = Some(self.session.clone());
        let prompt = ctx
            .store
            .prompts_dir()
            .join(format!("{}-{}-changes.md", task.id, run.id));
        std::fs::write(
            &prompt,
            format!(
                "{}\n\n## Current changes requested\n\n{text}\n",
                crate::task_prompt(ctx, task, &selected)?
            ),
        )
        .context("writing continued worker prompt")?;
        let command = crate::prompt::render_command_noticed(
            ctx,
            &selected.agent,
            task,
            &selected,
            &prompt,
            notice,
        )
        .map_err(|error| {
            if self.session.policy == SessionPolicy::Reuse {
                crate::role_session::SessionUnavailable(format!(
                    "worker context cannot be resumed: {error:#}"
                ))
                .into()
            } else {
                error
            }
        })?;
        let herdr = crate::gate::open_run_pane(ctx, task, run, "worker")?;
        Ok(Some(Prepared {
            herdr,
            command,
            old_pane: run.herdr.as_ref().map(|old| old.pane_id.clone()),
            receipt: crate::launch_exit::receipt(ctx, &task.id, &run.id),
            ctx: ctx.clone(),
            submitted: std::cell::Cell::new(false),
        }))
    }

    /// Runs only after the original capacity/repair admission has reopened the same run.
    pub fn start(&self, ctx: &Ctx, id: &str) -> Result<()> {
        if !self.restart {
            return Ok(());
        }
        crate::session_scope::check_launch(ctx)?;
        let task = ctx.store.get(id)?;
        let run = task.current_run().context("worker run disappeared")?;
        anyhow::ensure!(
            task.status == Status::Running && run.session.as_ref() == Some(&self.session),
            "worker continuation changed before submission"
        );
        let prompt = ctx
            .store
            .prompts_dir()
            .join(if crate::agent::hooks::run_is_pi_native(run) {
                format!("{id}-{}.md", run.id)
            } else {
                format!("{id}-{}-changes.md", run.id)
            });
        std::fs::write(&prompt, crate::task_prompt(ctx, &task, run)?)
            .context("writing continued worker prompt")?;
        if let Some(old) = &run.herdr
            && ctx.herdr.pane_get(&old.pane_id)?.is_some()
        {
            ctx.herdr.pane_close(&old.pane_id)?;
        }
        let herdr = crate::gate::open_run_pane(ctx, &task, run, "worker")?;
        let saved = ctx.store.update(id, |current| {
            let active = current
                .current_run_mut()
                .filter(|r| r.id == run.id)
                .context("worker changed during restart")?;
            active.herdr = Some(herdr.clone());
            active.inbox.binding = None;
            active.inbox.observe_cursor = Default::default();
            Ok(())
        })?;
        let active = saved
            .current_run()
            .context("worker disappeared after pane creation")?;
        let command = if crate::agent::hooks::run_is_pi_native(active) {
            crate::agent::hooks::launch::worker_command(ctx, &saved, active)?
        } else {
            crate::render_command(ctx, &active.agent, &saved, active, &prompt)?
        };
        let command =
            crate::launch_exit::arm(&command, &crate::launch_exit::receipt(ctx, id, &active.id))?;
        ctx.herdr.pane_run(&herdr.pane_id, &command)?;
        crate::role_session::observe(ctx, &saved, active, false);
        crate::role_session::log(ctx, &saved, active)
    }
}

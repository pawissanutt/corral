//! The auxiliary journal distinguishes prepared attachments from command submissions.
use anyhow::{Context, Result, ensure};
use corral_core::{CandidateIdentity, OperatingActionKind, Store, SubmissionDisposition, Task};

use super::{Kind, Pending, ReviewLaunch, clear, journal, save};
use crate::{Ctx, operating};

pub(super) fn action_kind(kind: Kind) -> OperatingActionKind {
    match kind {
        Kind::Gate => OperatingActionKind::Gate,
        Kind::Reviewer => OperatingActionKind::Review,
    }
}

pub(super) fn retained_path(ctx: &Ctx, task: &str, run: &str, kind: Kind) -> std::path::PathBuf {
    let key = match kind {
        Kind::Gate => "gate",
        Kind::Reviewer => "review",
    };
    ctx.store
        .state_dir()
        .join("review-prepared")
        .join(format!("{task}-{run}-{key}.json"))
}

pub(super) fn load_retained(
    ctx: &Ctx,
    task: &str,
    run: &str,
    kind: Kind,
) -> Result<Option<Pending>> {
    let path = retained_path(ctx, task, run, kind);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("parsing retained auxiliary claim")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("reading retained auxiliary claim"),
    }
}

/// Archive before releasing capacity. Repeating recovery is safe after any interrupted write.
pub(super) fn retain(ctx: &Ctx, pending: &Pending) -> Result<()> {
    let mut retained = pending.clone();
    if let Some(authorization) = &pending.authorization
        && let SubmissionDisposition::AuthorizationRefused { reason } = ctx
            .store
            .authorized_submission(&authorization.id)?
            .disposition
    {
        retained.refused = Some(reason);
    }
    let path = retained_path(ctx, &pending.task, &pending.run, pending.kind);
    if path.exists() {
        let previous: Pending = serde_json::from_slice(
            &std::fs::read(&path).context("reading retained review preparation")?,
        )
        .context("parsing retained review preparation")?;
        ensure!(
            previous.authorization.as_ref().map(|a| (&a.id, &a.action))
                == pending.authorization.as_ref().map(|a| (&a.id, &a.action)),
            "retained auxiliary attempt changed"
        );
        retained.prepared_gate = previous.prepared_gate;
        retained.prepared_review = previous.prepared_review;
    }
    std::fs::create_dir_all(path.parent().context("prepared review has no directory")?)
        .context("creating prepared review directory")?;
    ctx.store.update(&pending.task, |task| {
        if let Some(run) = task.runs.iter_mut().find(|run| run.id == pending.run) {
            let candidate = pending
                .authorization
                .as_ref()
                .and_then(|a| a.action.candidate.as_ref());
            match pending.kind {
                Kind::Gate => {
                    if run
                        .gate
                        .as_ref()
                        .is_some_and(|g| g.identity.as_ref() == candidate)
                    {
                        retained.prepared_gate = run.gate.clone();
                    }
                }
                Kind::Reviewer => {
                    if run
                        .review
                        .as_ref()
                        .is_some_and(|r| r.identity.as_ref() == candidate)
                        && run.review_runs.last().map(|r| &r.id) == pending.review_run.as_ref()
                    {
                        retained.prepared_review = run.review.clone();
                    }
                }
            }
            // Save all preparation before removing its active-process capacity marker.
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec(&retained)?)
                .context("writing retained review preparation")?;
            std::fs::rename(&tmp, &path).context("publishing retained review preparation")?;
            match pending.kind {
                Kind::Gate
                    if run.gate.as_ref() == retained.prepared_gate.as_ref()
                        && retained.prepared_gate.is_some() =>
                {
                    run.gate = None
                }
                Kind::Reviewer
                    if run.review.as_ref() == retained.prepared_review.as_ref()
                        && retained.prepared_review.is_some() =>
                {
                    run.review = None;
                    if let Some(child) = run
                        .review_runs
                        .iter_mut()
                        .find(|child| Some(&child.id) == pending.review_run.as_ref())
                    {
                        child.ended = Some(jiff::Timestamp::now());
                    }
                }
                Kind::Gate | Kind::Reviewer => {}
            }
        }
        task.review_reservation = None;
        Ok(())
    })?;
    clear(ctx)
}

impl ReviewLaunch<'_> {
    pub(crate) fn bound_candidate(&self) -> Option<CandidateIdentity> {
        self.pending
            .authorization
            .as_ref()
            .and_then(|a| a.action.candidate.clone())
    }

    pub(crate) fn bound_attempt(&self) -> Option<String> {
        self.pending
            .authorization
            .as_ref()
            .map(|a| a.action.attempt.clone())
    }

    /// Bind provenance and the exact candidate before preparing scripts, prompts or panes.
    pub(crate) fn bind(
        &mut self,
        candidate: &CandidateIdentity,
        attempt: &str,
        review_run: Option<&str>,
    ) -> Result<()> {
        let origin = match self.pending.automatic {
            Some(true) => operating::Origin::Automatic,
            Some(false) => operating::Origin::Owner,
            None => return Err(operating::missing_provenance(&self.pending.task)),
        };
        let mut identity = operating::identity(
            &self.pending.task,
            &self.pending.run,
            action_kind(self.pending.kind),
            attempt,
        );
        identity.candidate = Some(candidate.clone());
        self.pending.authorization = Some(operating::authorize(self.ctx, identity, origin)?);
        self.pending.review_run = review_run.map(str::to_owned);
        self.begin()
    }

    /// The caller supplies fresh candidate/record checks, never a nested board mutation.
    pub(crate) fn submit<R>(
        &mut self,
        prepare: impl FnOnce(&Store, &mut Task) -> Result<()>,
        send: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        let authorization = self
            .pending
            .authorization
            .clone()
            .context("review has no authorization")?;
        let result = operating::submit(
            self.ctx,
            &authorization,
            |store, task| {
                let bytes = std::fs::read(journal(self.ctx))
                    .context("reading auxiliary submission journal")?;
                let current: Pending = serde_json::from_slice(&bytes)
                    .context("parsing auxiliary submission journal")?;
                ensure!(
                    current.authorization.as_ref() == Some(&authorization)
                        && current.task == task.id
                        && current.run == authorization.action.run
                        && current.kind == self.pending.kind
                        && current.automatic == self.pending.automatic
                        && current.review_run == self.pending.review_run
                        && current.pane == self.pending.pane
                        && current.submitted == Some(false),
                    "auxiliary submission identity changed"
                );
                let run = task.current_run().context("auxiliary run disappeared")?;
                ensure!(run.id == current.run, "auxiliary parent run changed");
                let candidate = authorization.action.candidate.as_ref();
                match current.kind {
                    Kind::Gate => ensure!(
                        run.gate.as_ref().is_some_and(|gate| {
                            gate.identity.as_ref() == candidate
                                && candidate.is_some_and(|identity| gate.sha == identity.head)
                                && gate.pane == current.pane
                                && gate.holds_slot()
                        }),
                        "prepared gate identity changed"
                    ),
                    Kind::Reviewer => ensure!(
                        run.review.as_ref().is_some_and(|review| {
                            review.identity.as_ref() == candidate
                                && candidate.is_some_and(|identity| review.sha == identity.head)
                                && review.pane == current.pane
                                && review.holds_slot()
                        }) && run.review_runs.last().is_some_and(|child| {
                            Some(&child.id) == current.review_run.as_ref()
                                && child.id == authorization.action.attempt
                                && child.review == run.review
                                && child.herdr.as_ref().is_some_and(|herdr| {
                                    Some(&herdr.pane_id) == current.pane.as_ref()
                                        && herdr.socket == current.socket
                                })
                                && child.ended.is_none()
                        }),
                        "prepared reviewer accounting identity changed"
                    ),
                }
                crate::session_scope::check_launch_in(self.ctx, store)?;
                prepare(store, task)?;
                self.pending.submitted = Some(true);
                save(self.ctx, &self.pending)
            },
            send,
        );
        match result {
            Err(error) if operating::refused(&error) => {
                self.pending.submitted = Some(false);
                self.pending.refused = Some(error.to_string());
                if let Err(retention) =
                    save(self.ctx, &self.pending).and_then(|()| retain(self.ctx, &self.pending))
                {
                    return Err(error.context(format!(
                        "retaining refused auxiliary preparation: {retention:#}"
                    )));
                }
                self.committed = true;
                Err(error)
            }
            result => result,
        }
    }
}

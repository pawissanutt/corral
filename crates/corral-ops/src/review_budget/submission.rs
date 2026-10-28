//! The auxiliary journal distinguishes prepared attachments from command submissions.
use anyhow::{Context, Result, ensure};
use corral_core::records::RecordArea;
use corral_core::{
    ActionAuthorization, CandidateIdentity, OperatingActionKind, Store, SubmissionDisposition,
    Task, TaskHeadMut, TaskRef,
};

use super::{GateRequest, Kind, Pending, ReviewLaunch, clear, save};
use crate::{Ctx, operating};

const PREPARED: RecordArea<Pending> = RecordArea::new("review-prepared");

pub(super) fn action_kind(kind: Kind) -> OperatingActionKind {
    match kind {
        Kind::Gate => OperatingActionKind::Gate,
        Kind::Reviewer => OperatingActionKind::Review,
    }
}

pub(super) fn clear_retained(ctx: &Ctx, task: &str, run: &str, kind: Kind) -> Result<()> {
    PREPARED.delete(&ctx.store, &retained_key(task, run, kind))
}

fn retained_key(task: &str, run: &str, kind: Kind) -> String {
    let kind = match kind {
        Kind::Gate => "gate",
        Kind::Reviewer => "review",
    };
    format!("{task}-{run}-{kind}")
}

pub(super) fn load_retained(
    ctx: &Ctx,
    task: &str,
    run: &str,
    kind: Kind,
) -> Result<Option<Pending>> {
    PREPARED.load(&ctx.store, &retained_key(task, run, kind))
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
    ctx.store.update(&pending.task, |task| {
        if let Some(previous) = load_retained(ctx, &pending.task, &pending.run, pending.kind)? {
            ensure!(
                previous.authorization.as_ref().map(|a| (&a.id, &a.action))
                    == pending.authorization.as_ref().map(|a| (&a.id, &a.action)),
                "retained auxiliary attempt changed"
            );
            retained.prepared_gate = previous.prepared_gate;
            retained.prepared_review = previous.prepared_review;
        }
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
        // Preparation, capacity release and active-journal cleanup commit in one UOW.
        PREPARED.save(
            &ctx.store,
            &retained_key(&pending.task, &pending.run, pending.kind),
            &retained,
        )?;
        task.review_reservation = None;
        clear(ctx)
    })?;
    Ok(())
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
    /// A bound agent request is fenced here, the boundary both gate paths reach: its liveness
    /// check runs first under the store's locks, and every refusal carries its request token.
    pub(crate) fn submit<R>(
        &mut self,
        prepare: impl FnOnce(&Store, &mut Task) -> Result<()>,
        send: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        let (request, authorization) = self.begin_submit()?;
        let ctx = self.ctx;
        let result = operating::submit(
            ctx,
            &authorization,
            |store, task| {
                with_request(request.as_ref(), || {
                    self.fence(request.as_ref(), &authorization, store, &*task)?;
                    prepare(store, task)?;
                    self.mark_submitted()
                })
            },
            || send_with(request.as_ref(), send),
        );
        self.finish_submit(request, result)
    }

    /// [`Self::submit`] with `prepare` mutating the task's head: the same fence, checks and
    /// refusal journal.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the head-form chain lands before T-253's action children convert its callers"
        )
    )]
    pub(crate) fn submit_head<R>(
        &mut self,
        prepare: impl FnOnce(&Store, &mut TaskHeadMut) -> Result<()>,
        send: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        let (request, authorization) = self.begin_submit()?;
        let ctx = self.ctx;
        let result = operating::submit_head(
            ctx,
            &authorization,
            |store, head| {
                with_request(request.as_ref(), || {
                    self.fence(request.as_ref(), &authorization, store, &*head)?;
                    prepare(store, head)?;
                    self.mark_submitted()
                })
            },
            || send_with(request.as_ref(), send),
        );
        self.finish_submit(request, result)
    }

    fn begin_submit(&self) -> Result<(Option<GateRequest>, ActionAuthorization)> {
        let request = self.request.clone();
        if request.is_some() {
            crate::agent::failpoint("gate_submit_before_lock");
        }
        let authorization = self
            .pending
            .authorization
            .clone()
            .context("review has no authorization")?;
        Ok((request, authorization))
    }

    fn finish_submit<R>(&mut self, request: Option<GateRequest>, result: Result<R>) -> Result<R> {
        match result {
            Err(error) if operating::refused(&error) => {
                if request.is_some() {
                    crate::agent::failpoint("gate_refusal_before_retain");
                }
                self.pending.submitted = Some(false);
                self.pending.refused = Some(error.to_string());
                self.pending.refused_request = request.map(|r| r.request_id);
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

    /// The one submission fence of both forms: a bound request's liveness first, then the
    /// journal identity, the parent run and the prepared record.
    fn fence<T: TaskRef + ?Sized>(
        &self,
        request: Option<&GateRequest>,
        authorization: &ActionAuthorization,
        store: &Store,
        task: &T,
    ) -> Result<()> {
        if let Some(request) = request {
            corral_core::agent::check_live_ref(task, &request.binding)??;
        }
        let current: Pending = store
            .record("review-launch", "current")?
            .context("auxiliary submission journal disappeared")?;
        ensure!(
            current.authorization.as_ref() == Some(authorization)
                && current.task == *task.id()
                && current.run == authorization.action.run
                && current.kind == self.pending.kind
                && current.automatic == self.pending.automatic
                && current.review_run == self.pending.review_run
                && current.pane == self.pending.pane
                && current.submitted == Some(false),
            "auxiliary submission identity changed"
        );
        let run = task
            .current_run_head()?
            .context("auxiliary run disappeared")?;
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
                }) && run.current_child.as_deref().is_some_and(|child| {
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
        crate::session_scope::check_launch_in(self.ctx, store)
    }

    fn mark_submitted(&mut self) -> Result<()> {
        self.pending.submitted = Some(true);
        save(self.ctx, &self.pending)
    }
}

fn with_request(request: Option<&GateRequest>, f: impl FnOnce() -> Result<()>) -> Result<()> {
    match request {
        Some(request) => f().with_context(|| format!("agent_request={}", request.request_id)),
        None => f(),
    }
}

fn send_with<R>(request: Option<&GateRequest>, send: impl FnOnce() -> Result<R>) -> Result<R> {
    if request.is_some() {
        crate::agent::failpoint("gate_submit_ipc");
    }
    send()
}

#[cfg(all(test, unix))]
mod fence_tests {
    use corral_core::agent::{ContainerRef, RequestBinding};
    use corral_core::{Status, Verdict};
    use serde_json::json;

    use super::{GateRequest, Kind, ReviewLaunch, load_retained};
    use crate::operating::Origin;
    use crate::submission_paths_tests::fixture;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Case {
        Live,
        Fenced,
        RunEnded,
        ReviewClosed,
        FencedWithChangedRecord,
    }

    fn agent_host(role: &str, fenced: bool, callback: Option<&str>) -> serde_json::Value {
        json!({
            "version": 1,
            "binding": {"role": role, "store": "s", "project": "app", "generation": 1,
                        "health": "healthy", "callback": callback},
            "generations": [{"n": 1, "bind_nonce_sha256": "0".repeat(64), "pane": "p-1",
                             "created": fixture::now(),
                             "fenced": if fenced { json!({"reason": "lost", "at": fixture::now()}) } else { json!(null) }}],
            "turns": [{"generation": 1, "seq": 2, "origin": {"kind": "bridge"},
                       "started": fixture::now(), "ended": null}]
        })
    }

    /// One bound gate submission through `head` or the full form; the outcome, send count and
    /// journaled refused request.
    fn submit(case: Case, head: bool) -> (Result<(), String>, usize, Option<String>) {
        let fx = fixture::Fx::new();
        let task = fx.task("fence", Status::Ready);
        let id = task.id;
        fx.start(&id);
        fx.done(&id, "candidate.txt");
        let task = fx.ctx.store.get(&id).unwrap();
        let run = task.current_run().unwrap().clone();
        let candidate =
            crate::verification::capture_identity(&fx.ctx, &task, &run, "candidate").unwrap();
        let pane = run.herdr.clone().unwrap();
        let mut launch =
            ReviewLaunch::reserve_origin(&fx.ctx, &id, Kind::Gate, false, Origin::Owner).unwrap();
        launch.bind(&candidate, "candidate", None).unwrap();
        launch.attach(&pane.pane_id, true).unwrap();
        fx.ctx
            .store
            .update(&id, |task| {
                let run = task.current_run_mut().unwrap();
                run.gate = Some(serde_json::from_value(json!({
                    "result": "running", "sha": candidate.head, "at": fixture::now(),
                    "pane": if case == Case::FencedWithChangedRecord { json!("moved") } else { json!(pane.pane_id) },
                    "socket": pane.socket, "identity": candidate
                }))?);
                let fenced = matches!(case, Case::Fenced | Case::FencedWithChangedRecord);
                run.agent_host = Some(serde_json::from_value(agent_host("worker", fenced, None))?);
                let mut child = run.clone();
                child.id = "R1-review1".into();
                child.review_runs.clear();
                child.agent_host =
                    Some(serde_json::from_value(agent_host("reviewer", false, Some("cb")))?);
                child.review = Some(serde_json::from_value(json!({
                    "reviewer": "rev", "verdict": Verdict::Approve, "notes": null,
                    "sha": candidate.head, "at": fixture::now()
                }))?);
                run.review_runs.push(child);
                Ok(())
            })
            .unwrap();
        let container = match case {
            Case::RunEnded => ContainerRef::Worker {
                task: id.clone(),
                run: "R9".into(),
            },
            Case::ReviewClosed => ContainerRef::Reviewer {
                task: id.clone(),
                run: run.id.clone(),
                review: "R1-review1".into(),
            },
            _ => ContainerRef::Worker {
                task: id.clone(),
                run: run.id.clone(),
            },
        };
        launch.bind_request(GateRequest {
            request_id: "req-1".into(),
            attempt: "candidate".into(),
            binding: RequestBinding {
                container,
                generation: 1,
                callback: (case == Case::ReviewClosed).then(|| "cb".to_owned()),
                settles_turn: Some(2),
            },
        });
        let sends = std::cell::Cell::new(0);
        let send = || {
            sends.set(sends.get() + 1);
            Ok(())
        };
        let refused_prepare = || -> anyhow::Result<()> {
            assert_eq!(
                case,
                Case::Live,
                "{case:?}: prepare ran after a fence refusal"
            );
            Ok(())
        };
        let result = if head {
            launch.submit_head(|_, _| refused_prepare(), send)
        } else {
            launch.submit(|_, _| refused_prepare(), send)
        };
        let result = result.map_err(|error| format!("{error:#}"));
        if result.is_ok() {
            launch.commit().unwrap();
        } else {
            drop(launch);
        }
        let refused_request = load_retained(&fx.ctx, &id, &run.id, Kind::Gate)
            .unwrap()
            .and_then(|pending| pending.refused_request);
        (result, sends.get(), refused_request)
    }

    #[test]
    fn both_submit_forms_run_the_one_fence_first_with_identical_refusals() {
        for (case, code) in [
            (Case::Live, None),
            (Case::Fenced, Some("agent_fenced")),
            (Case::RunEnded, Some("run_ended")),
            (Case::ReviewClosed, Some("review_closed")),
            (Case::FencedWithChangedRecord, Some("agent_fenced")),
        ] {
            let full = submit(case, false);
            let head = submit(case, true);
            assert_eq!(full, head, "{case:?}");
            let (result, sends, refused_request) = full;
            match code {
                None => {
                    assert_eq!(result, Ok(()), "{case:?}");
                    assert_eq!(sends, 1, "{case:?}: a live binding submits once");
                    assert_eq!(refused_request, None);
                }
                Some(code) => {
                    let error = result.unwrap_err();
                    assert!(error.contains("agent_request=req-1"), "{case:?}: {error}");
                    assert!(
                        error.contains(&format!("agent_refusal={code}")),
                        "{case:?}: {error}"
                    );
                    assert!(!error.contains("identity changed"), "{case:?}: {error}");
                    assert_eq!(sends, 0, "{case:?}: a refusal sent");
                    assert_eq!(refused_request.as_deref(), Some("req-1"), "{case:?}");
                }
            }
        }
    }
}

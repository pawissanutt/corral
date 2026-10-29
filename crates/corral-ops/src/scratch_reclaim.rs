//! Attempt scratch directories (plan T-253 section 8): implementation, design-plan and aggregate
//! review attempts and planner runs each get one registered directory below the scratch root,
//! never under `.corral`. A bounded pass at sync closes the directories of attempts that ended
//! (verdict, supersession, lost pane, owner decision), warns once per open attempt that
//! outgrows `review_scratch_warn`, and reclaims closed directories only while nothing uses
//! them; otherwise it defers them with a reason and a doubling recheck delay.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use corral_core::backend::{ScratchKind, ScratchRow, ScratchState};
use corral_core::inbox::{
    DeliveryMode, Inbox, MAX_PAYLOAD, MessageBinding, Post, Recipient, Urgency,
};
use corral_core::records::RecordArea;
use corral_core::scratch::AttemptScratch;
use corral_core::{PlanRunState, Status};
use corral_herdr::AgentStatus;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Ctx;

/// Stable across restarts so the next pass completes a reclaim a crashed pass began; the
/// `scratch-reclaim` lease keeps passes from overlapping.
pub(crate) const OWNER: &str = "scratch-reclaim";
pub(crate) const LEASE: &str = "scratch-reclaim";
const BATCH: usize = 16;
const FIRST_DEFER: Duration = Duration::from_secs(60);
const MAX_DEFER: Duration = Duration::from_secs(3600);
/// A registered attempt whose record never appeared (a launch that failed before saving it)
/// counts as ended after this long.
const UNRECORDED: SignedDuration = SignedDuration::from_hours(24);

/// One attempt that owns a scratch directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attempt<'a> {
    Review {
        task: &'a str,
        run: &'a str,
        child: &'a str,
    },
    PlanReview {
        task: &'a str,
        plan: &'a str,
        review: &'a str,
    },
    Aggregate {
        task: &'a str,
        attempt: &'a str,
        review: &'a str,
    },
    Planner {
        task: &'a str,
        run: &'a str,
    },
}

impl<'a> Attempt<'a> {
    /// The attempt a registration belongs to; `None` for spools and gate or prompt directories.
    fn of(row: &'a ScratchRow) -> Option<Self> {
        let task = row.task.as_deref()?;
        let attempt = row.attempt.as_deref()?;
        let subject = row.subject.as_deref();
        Some(match row.kind {
            ScratchKind::ImplementationReview => Self::Review {
                task,
                run: subject?,
                child: attempt,
            },
            ScratchKind::DesignPlanReview => Self::PlanReview {
                task,
                plan: subject?,
                review: attempt,
            },
            ScratchKind::AggregateReview => Self::Aggregate {
                task,
                attempt: subject?,
                review: attempt,
            },
            ScratchKind::PlannerOutput => Self::Planner { task, run: attempt },
            _ => return None,
        })
    }

    fn kind(&self) -> ScratchKind {
        match self {
            Self::Review { .. } => ScratchKind::ImplementationReview,
            Self::PlanReview { .. } => ScratchKind::DesignPlanReview,
            Self::Aggregate { .. } => ScratchKind::AggregateReview,
            Self::Planner { .. } => ScratchKind::PlannerOutput,
        }
    }

    /// (registration prefix, class directory, task, subject, attempt, unique name)
    fn parts(
        &self,
    ) -> (
        &'static str,
        &'static str,
        &'a str,
        Option<&'a str>,
        &'a str,
        String,
    ) {
        match *self {
            Self::Review { task, run, child } => (
                "review",
                "reviews",
                task,
                Some(run),
                child,
                format!("{task}-{child}"),
            ),
            Self::PlanReview { task, plan, review } => (
                "plan-review",
                "plan-reviews",
                task,
                Some(plan),
                review,
                format!("{task}-{plan}-{review}"),
            ),
            Self::Aggregate {
                task,
                attempt,
                review,
            } => (
                "aggregate-review",
                "aggregate-reviews",
                task,
                Some(attempt),
                review,
                format!("{task}-{review}"),
            ),
            Self::Planner { task, run } => {
                ("planner", "plans", task, None, run, format!("{task}-{run}"))
            }
        }
    }

    pub(crate) fn id(&self) -> String {
        let (prefix, _, _, _, _, name) = self.parts();
        format!("{prefix}-{name}")
    }

    fn rel(&self) -> String {
        let (_, class, _, _, _, name) = self.parts();
        format!("{class}/{name}")
    }

    /// The attempt's directory below the scratch root; nothing is read or created.
    pub(crate) fn dir(&self, ctx: &Ctx) -> Result<PathBuf> {
        ctx.store.scratch_path(&self.rel())
    }

    /// Registers and creates the attempt's directory, or returns it while it is still open.
    pub(crate) fn open(&self, ctx: &Ctx) -> Result<PathBuf> {
        let (_, _, task, subject, attempt, _) = self.parts();
        let (id, rel) = (self.id(), self.rel());
        ctx.store.attempt_scratch(&AttemptScratch {
            kind: self.kind(),
            id: &id,
            task,
            subject,
            attempt,
            rel: &rel,
        })
    }

    /// Makes the attempt's directory eligible for reclaim; a no-op once it is closed.
    pub(crate) fn close(&self, ctx: &Ctx) -> Result<()> {
        ctx.store.close_open_scratch(&self.id()).map(drop)
    }
}

/// The prompt section naming an attempt's scratch directory and its evidence budget.
pub(crate) fn prompt_section(ctx: &Ctx, dir: &Path) -> String {
    format!(
        "\n## Scratch directory and evidence budget\n\n\
         Your scratch directory for this attempt is {}. Keep notes, screenshots and other \
         working files there; never write under the workspace's .corral directory. It is \
         reclaimed after the attempt ends. Evidence you attach with `--evidence PATH` \
         (repeatable) must be regular files inside this directory or the reviewed worktree, \
         at most {} in total.\n",
        dir.display(),
        ctx.store.config.storage.review_evidence_max,
    )
}

struct State {
    ended: bool,
    pane: Option<String>,
    /// The attempt's keyed inbox, when it was launched with inbox delivery.
    inbox: Option<Recipient>,
}

fn state(ctx: &Ctx, row: &ScratchRow, attempt: Attempt<'_>, now: Timestamp) -> Result<State> {
    let (_, _, task, _, _, _) = attempt.parts();
    let head = ctx.store.head(task)?;
    let unrecorded = || State {
        ended: now.duration_since(row.created) > UNRECORDED,
        pane: None,
        inbox: None,
    };
    let pane = |herdr: Option<corral_core::HerdrRef>| herdr.map(|h| h.pane_id);
    let mut found = match attempt {
        Attempt::Review { run, child, .. } => {
            let current = head
                .current_run
                .as_ref()
                .filter(|current| current.id == run);
            match current {
                None => State {
                    ended: true,
                    pane: None,
                    inbox: None,
                },
                Some(current) => {
                    // `child` keys the directory: the callback attempt of the review using it.
                    let latest = current.current_child.as_deref();
                    let latest_key = latest.map(|latest| {
                        crate::reviewer::dir_attempt(latest.review.as_ref(), &latest.id)
                    });
                    let current_key = current.review.as_ref().map(|review| {
                        crate::reviewer::dir_attempt(Some(review), latest.map_or("", |l| &l.id))
                    });
                    // The review of a run still in Review keeps its directory until a verdict or
                    // a newer attempt, including after a refused or interrupted launch.
                    let open = head.task.status == Status::Review
                        && current.review.as_ref().is_none_or(|r| r.verdict.is_none())
                        && [current_key, latest_key]
                            .iter()
                            .flatten()
                            .any(|key| key == child);
                    if open {
                        State {
                            ended: false,
                            pane: latest.and_then(|l| l.herdr.clone()).map(|h| h.pane_id),
                            inbox: latest
                                .filter(|l| l.delivery == DeliveryMode::Inbox)
                                .map(|l| Recipient::ReviewRun {
                                    task: task.to_owned(),
                                    run: run.to_owned(),
                                    review: l.id.clone(),
                                }),
                        }
                    } else if head.review_child(run, child)?.is_none()
                        && current.review.as_ref().is_none_or(|r| r.verdict.is_none())
                    {
                        unrecorded()
                    } else {
                        State {
                            ended: true,
                            pane: latest.and_then(|l| l.herdr.clone()).map(|h| h.pane_id),
                            inbox: None,
                        }
                    }
                }
            }
        }
        Attempt::PlanReview { plan, review, .. } => match head.plan_review(plan, review)? {
            Some(found) => State {
                ended: found.verdict.is_some(),
                inbox: (found.delivery == DeliveryMode::Inbox).then(|| Recipient::PlanReview {
                    task: task.to_owned(),
                    plan: plan.to_owned(),
                    review: review.to_owned(),
                }),
                pane: pane(found.herdr),
            },
            None if crate::plan_review::retains(ctx, task, plan, review)? => State {
                ended: false,
                pane: None,
                inbox: None,
            },
            None => unrecorded(),
        },
        Attempt::Aggregate {
            attempt, review, ..
        } => match head.aggregate_review(attempt, review)? {
            Some(review) => State {
                ended: review.verdict.is_some() || review.abandoned.is_some(),
                pane: pane(review.herdr),
                inbox: None,
            },
            None => unrecorded(),
        },
        Attempt::Planner { run, .. } => match head.last_plan_run.as_ref() {
            Some(current) if current.id == run => State {
                // A failed run that already submitted has nothing left to write; the planner
                // failure path closes a run that failed while planning once its evidence is in.
                ended: matches!(
                    current.state,
                    PlanRunState::Approved | PlanRunState::Cancelled
                ) || (current.state == PlanRunState::Failed && current.submitted.is_some()),
                pane: pane(current.herdr.clone()),
                inbox: (current.delivery == DeliveryMode::Inbox).then(|| Recipient::PlanRun {
                    task: task.to_owned(),
                    plan_run: run.to_owned(),
                }),
            },
            Some(current) if ordinal(&current.id) > ordinal(run) => State {
                ended: true,
                pane: None,
                inbox: None,
            },
            _ => unrecorded(),
        },
    };
    if matches!(head.task.status, Status::Done | Status::Dropped) {
        found.ended = true;
    }
    Ok(found)
}

/// The number of planner run `D<n>`.
fn ordinal(run: &str) -> u64 {
    run.trim_start_matches('D').parse().unwrap_or(0)
}

/// Bytes below `dir`, without following symlinks.
pub(crate) fn usage(dir: &Path) -> Result<u64> {
    let mut total = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let entries = match std::fs::read_dir(&next) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("listing {}", next.display())),
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", next.display()))?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    Ok(total)
}

/// Why the directory of a closed attempt is still in use: its pane is still working, a Cargo
/// build holds a `.cargo-lock` under it, or a process has its working directory or an open file
/// under it. Other platforms cannot see processes, so they always defer.
fn in_use(ctx: &Ctx, pane: Option<&str>, dir: &Path) -> Result<Option<String>> {
    if let Some(pane) = pane
        && ctx
            .herdr
            .pane_get(pane)?
            .is_some_and(|info| info.status == AgentStatus::Working)
    {
        return Ok(Some(format!("pane {pane} is still working")));
    }
    let dir = match std::fs::canonicalize(dir) {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("resolving {}", dir.display())),
    };
    if cfg!(not(target_os = "linux")) {
        return Ok(Some(
            "process use cannot be checked on this platform".into(),
        ));
    }
    if let Some(lock) = held_cargo_lock(&dir)? {
        return Ok(Some(lock));
    }
    dir_user(&dir)
}

fn report(row: &ScratchRow, error: &anyhow::Error) {
    crate::messages::report(
        row.task.as_deref(),
        "scratch",
        format!("scratch {}: {error:#}", row.id),
    );
}

/// One bounded pass at `now` (see the module documentation). Per-directory failures are
/// reported and never stop the pass; another process's running pass makes this one a no-op.
pub fn reclaim_scratch_pass(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let Some(_lease) = ctx.store.try_lock(LEASE)? else {
        return Ok(());
    };
    for row in ctx.store.scratch_rows_in(&[ScratchState::Open])? {
        if let Some(attempt) = Attempt::of(&row)
            && let Err(error) = review_open(ctx, &row, attempt, now)
        {
            report(&row, &error);
        }
    }
    for row in ctx.store.scratch_rows_in(&[ScratchState::Reclaiming])? {
        if row.owner.as_deref() == Some(OWNER)
            && let Err(error) = resume(ctx, &row, now)
        {
            report(&row, &error);
        }
    }
    let due = ctx
        .store
        .scratch_rows_in(&[ScratchState::Closed, ScratchState::Deferred])?
        .into_iter()
        .filter(|row| row.next_check_at.is_none_or(|at| at <= now))
        .take(BATCH);
    for row in due {
        if let Err(error) = reclaim(ctx, &row, now) {
            report(&row, &error);
        }
    }
    Ok(())
}

/// Where the steering message owed to an attempt that crossed `review_scratch_warn` stands. It
/// is recorded before the once-only warning event, so a missing pane or a definite refusal only
/// delays it. Keyed inbox delivery owns it once posted. On the plain prompt transport a send is
/// marked `Submitting` first: an interrupted send cannot tell accepted from unsent, so it becomes
/// `Uncertain` with one alert and is never resent (owner decision 2026-10-27: at most once).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Owed,
    Submitting,
    Delivered,
    Posted,
    Uncertain,
}

#[derive(Serialize, Deserialize)]
struct OwedWarning {
    message: String,
    stage: Stage,
}

const WARNINGS: RecordArea<OwedWarning> = RecordArea::new("scratch-warnings");

/// Closes an ended attempt's directory; for an active one, warns once when it outgrew its budget
/// and hands the owed steering message to its transport.
fn review_open(ctx: &Ctx, row: &ScratchRow, attempt: Attempt<'_>, now: Timestamp) -> Result<()> {
    let state = state(ctx, row, attempt, now)?;
    if state.ended {
        return attempt.close(ctx);
    }
    let mut owed = WARNINGS.load(&ctx.store, &row.id)?;
    if row.warned.is_none() {
        let limit = ctx.store.config.storage.review_scratch_warn_bytes()?;
        let dir = attempt.dir(ctx)?;
        let bytes = usage(&dir)?;
        if bytes >= limit {
            if owed.is_none() {
                let warning = OwedWarning {
                    message: format!(
                        "corral: your scratch directory {} holds {bytes} bytes, over \
                         review_scratch_warn ({}). Remove files you no longer need; attach only \
                         the evidence your verdict needs.",
                        dir.display(),
                        ctx.store.config.storage.review_scratch_warn,
                    ),
                    stage: Stage::Owed,
                };
                WARNINGS.save(&ctx.store, &row.id, &warning)?;
                owed = Some(warning);
            }
            ctx.store.warn_scratch(&row.id, bytes)?;
        }
    }
    let Some(warning) = owed else {
        return Ok(());
    };
    match warning.stage {
        Stage::Owed => deliver(ctx, row, &state, warning),
        Stage::Submitting => {
            let uncertain = OwedWarning {
                stage: Stage::Uncertain,
                ..warning
            };
            WARNINGS.save(&ctx.store, &row.id, &uncertain)?;
            ctx.store.log_event(
                row.task.as_deref(),
                "storage_scratch_warning_uncertain",
                serde_json::json!({"scratch": row.id, "pane": state.pane}),
            )
        }
        Stage::Delivered | Stage::Posted | Stage::Uncertain => Ok(()),
    }
}

/// Sends an owed warning: posted once to the attempt's keyed inbox, or prompted to its pane at
/// most once.
fn deliver(ctx: &Ctx, row: &ScratchRow, state: &State, warning: OwedWarning) -> Result<()> {
    if let Some(recipient) = &state.inbox {
        let inbox = Inbox::new(&ctx.store, recipient.clone())?;
        let post = Post {
            kind: "scratch_warning",
            urgency: Urgency::Wake,
            author: "corral",
            body: &warning.message,
            binding: MessageBinding::default(),
            subject: Some(serde_json::json!({"scratch": row.id})),
            max_payload: MAX_PAYLOAD,
        };
        let posted = OwedWarning {
            stage: Stage::Posted,
            message: warning.message.clone(),
        };
        // The record joins the post's own update: the message is posted exactly once.
        inbox.post_with(post, |_, _| WARNINGS.save(&ctx.store, &row.id, &posted))?;
        return Ok(());
    }
    let Some(pane) = state.pane.as_deref() else {
        return Ok(());
    };
    let submitting = OwedWarning {
        stage: Stage::Submitting,
        message: warning.message,
    };
    WARNINGS.save(&ctx.store, &row.id, &submitting)?;
    let delivery = ctx.herdr.agent_prompt(pane, &submitting.message)?;
    // Only a proven non-submission returns to Owed: the message still waits in the pane's
    // input (a retry submits it without pasting again), or the terminal path never sent Enter.
    // Any other unconfirmed outcome, such as a transport that died after the engine accepted
    // the prompt, stays Submitting and becomes Uncertain, never resent.
    let proven_unsent = !delivery.submitted
        && ((delivery.via == corral_herdr::DeliveryVia::SendText && delivery.attempts == 0)
            || ctx
                .herdr
                .input_pending(pane, &submitting.message)
                .unwrap_or(false));
    let stage = if delivery.submitted {
        Stage::Delivered
    } else if proven_unsent {
        Stage::Owed
    } else {
        Stage::Submitting
    };
    WARNINGS.save(
        &ctx.store,
        &row.id,
        &OwedWarning {
            stage,
            ..submitting
        },
    )?;
    anyhow::ensure!(
        delivery.submitted,
        "steering message to pane {pane} was not confirmed ({}): {}",
        if proven_unsent {
            "retrying next pass"
        } else {
            "left uncertain, never resent"
        },
        delivery.error.as_deref().unwrap_or("unconfirmed")
    );
    Ok(())
}

/// The pane of the attempt owning registration `row`, if it recorded one.
fn pane_of(ctx: &Ctx, row: &ScratchRow, now: Timestamp) -> Result<Option<String>> {
    match Attempt::of(row) {
        Some(attempt) => Ok(state(ctx, row, attempt, now)?.pane),
        None => Ok(None),
    }
}

/// Completes a reclaim an earlier pass began and never recorded. A directory still present is
/// checked like any other first: while something uses it the reclaim stays pending (a begun
/// reclaim cannot be deferred) and the next pass checks again.
fn resume(ctx: &Ctx, row: &ScratchRow, now: Timestamp) -> Result<()> {
    let dir = ctx.store.scratch_path(&row.path)?;
    if let Some(reason) = in_use(ctx, pane_of(ctx, row, now)?.as_deref(), &dir)? {
        crate::messages::report(
            row.task.as_deref(),
            "scratch",
            format!(
                "scratch {} resumes reclaiming once unused: {reason}",
                row.id
            ),
        );
        return Ok(());
    }
    ctx.store.reclaim_scratch_entry(&row.id, OWNER).map(drop)
}

/// Why the closed or deferred directory of `row` is still in use, read-only.
pub(crate) fn in_use_now(ctx: &Ctx, row: &ScratchRow, now: Timestamp) -> Result<Option<String>> {
    let dir = ctx.store.scratch_path(&row.path)?;
    in_use(ctx, pane_of(ctx, row, now)?.as_deref(), &dir)
}

/// Reclaims `row` when nothing uses it, else defers it; returns the reason it was deferred.
/// The caller holds [`LEASE`].
pub(crate) fn reclaim(ctx: &Ctx, row: &ScratchRow, now: Timestamp) -> Result<Option<String>> {
    let Some(reason) = in_use_now(ctx, row, now)? else {
        ctx.store.reclaim_scratch_entry(&row.id, OWNER)?;
        return Ok(None);
    };
    let delay = match (row.state, row.closed) {
        (ScratchState::Deferred, Some(closed)) => now
            .duration_since(closed)
            .unsigned_abs()
            .clamp(FIRST_DEFER, MAX_DEFER),
        _ => FIRST_DEFER,
    };
    let next = now.checked_add(SignedDuration::try_from(delay)?)?;
    ctx.store.defer_scratch(&row.id, row.state, &reason, next)?;
    Ok(Some(reason))
}

/// A process whose working directory or an open file lies under `dir`; processes that exit or
/// cannot be inspected mid-scan use nothing visible. Linux only (`/proc`).
pub(crate) fn dir_user(dir: &Path) -> Result<Option<String>> {
    let processes = std::fs::read_dir("/proc").context("listing /proc")?;
    for process in processes.flatten() {
        let name = process.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let base = process.path();
        let mut links: Vec<PathBuf> = vec![base.join("cwd")];
        // This process's own descriptors are the deleter's handles on the directory itself.
        let own = name.to_string_lossy() == std::process::id().to_string();
        // A process that exits or is not ours to inspect mid-scan uses nothing we can see.
        if let Ok(fds) = std::fs::read_dir(base.join("fd"))
            && !own
        {
            links.extend(fds.flatten().map(|fd| fd.path()));
        }
        if let Some(path) = links
            .iter()
            .filter_map(|link| std::fs::read_link(link).ok())
            .find(|target| target.starts_with(dir))
        {
            return Ok(Some(format!(
                "process {} uses {}",
                name.to_string_lossy(),
                path.display()
            )));
        }
    }
    Ok(None)
}

/// A `.cargo-lock` under `dir` that another open file description holds (a running build).
pub(crate) fn held_cargo_lock(dir: &Path) -> Result<Option<String>> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let entries = match std::fs::read_dir(&next) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("listing {}", next.display())),
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", next.display()))?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.file_name() == ".cargo-lock" {
                let file = std::fs::File::open(entry.path())?;
                match file.try_lock() {
                    Ok(()) => file.unlock()?,
                    Err(std::fs::TryLockError::WouldBlock) => {
                        return Ok(Some(format!("{} is held", entry.path().display())));
                    }
                    Err(std::fs::TryLockError::Error(error)) => {
                        return Err(error)
                            .with_context(|| format!("locking {}", entry.path().display()));
                    }
                }
            }
        }
    }
    Ok(None)
}

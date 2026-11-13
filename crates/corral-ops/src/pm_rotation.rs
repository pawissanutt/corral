//! Fresh-conversation PM rotation and its barrier (Part 2 D).
//!
//! A relaunch continues the PM's conversation in a new generation; a rotation opens a new
//! generation with a fresh Pi session, the answer to context growth. Work that lives only in
//! the old conversation would be lost with it, so rotation decides in two steps under the
//! `pm-recovery` lease ("freeze, then decide"): a pre-check of records and the bound journal
//! refuses without changing anything; when it is clear, the predecessor is stopped and fenced
//! like `relaunch --replace` and the barrier is read again from that frozen evidence. Anything
//! owed then keeps the conversation (an ordinary relaunch), so the worst case costs one
//! restart of the same conversation, never a lost one. A host that merely looks idle is never
//! sufficient. Nothing rotates automatically; only the owner's `corral pm rotate` calls it.
use std::cell::RefCell;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::StepState;
use corral_core::agent::{AuxiliaryOutcome, JobState, UsageSessionKind, pm_state};
use corral_core::inbox::{Inbox, ReceiptLevel, Recipient};
use serde::Serialize;
use serde_json::json;

use crate::Ctx;
use crate::agent::recovery::PmRecoveryLease;

#[path = "pm_rotation_journal.rs"]
mod journal;

/// An owner dialog the old conversation still owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogState {
    /// Waiting for the owner's choice.
    Pending,
    /// Answered, but the answer is not yet a user message of the conversation.
    AnsweredUndelivered,
}

/// What a fresh conversation would lose or interrupt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Blocker {
    /// Another recovery entry opened this generation; it has not completed hello.
    SuccessorStarting {
        generation: u32,
    },
    OpenTurn {
        generation: u32,
        turn: u64,
    },
    BoardQuestion {
        key: String,
    },
    OwnerDialog {
        id: String,
        state: DialogState,
    },
    /// Pi input of this generation still waiting in its steer/follow-up queue.
    QueuedInput {
        item: String,
        origin: String,
        mode: String,
    },
    /// A delivery the host accepted (its `corral_delivery` entry) that never became a turn.
    UnconsumedDelivery {
        key: String,
    },
    JournalUnreadable {
        file: PathBuf,
        error: String,
    },
    InboxMessage {
        id: String,
        kind: String,
    },
    OpenEnvelope {
        envelope: u64,
    },
    Job {
        id: String,
        state: JobState,
    },
    Monitor {
        job: String,
        wait: String,
    },
    AuxiliaryChild {
        kind: UsageSessionKind,
        session_id: String,
    },
    PlanStep {
        plan: Option<String>,
        step: String,
        label: String,
    },
    UsageGuardPaused {
        generation: u32,
    },
}

impl Blocker {
    /// Blockers even the owner's `--force` cannot override: fencing would cut a turn, queued
    /// input or a delivery in flight, the journal that proves otherwise cannot be read, or a
    /// successor another entry is starting would be replaced.
    pub fn hard(&self) -> bool {
        matches!(
            self,
            Self::SuccessorStarting { .. }
                | Self::OpenTurn { .. }
                | Self::QueuedInput { .. }
                | Self::UnconsumedDelivery { .. }
                | Self::JournalUnreadable { .. }
                | Self::OpenEnvelope { .. }
        )
    }

    pub fn sentence(&self) -> String {
        match self {
            Self::SuccessorStarting { generation } => format!(
                "generation {generation} is starting and has not completed hello; retry once \
                 it is up"
            ),
            Self::OpenTurn { generation, turn } => {
                format!("generation {generation} has turn {turn} open")
            }
            Self::BoardQuestion { key } => format!("board question {key} is still pending"),
            Self::OwnerDialog {
                id,
                state: DialogState::Pending,
            } => format!("owner dialog {id} is waiting for the owner's choice"),
            Self::OwnerDialog {
                id,
                state: DialogState::AnsweredUndelivered,
            } => format!("the owner's answer to dialog {id} has not reached the conversation"),
            Self::QueuedInput { item, origin, mode } => {
                format!("{origin} input {item} is still queued ({mode}) in the conversation")
            }
            Self::UnconsumedDelivery { key } => {
                format!("delivery {key} reached the host but never became part of a turn")
            }
            Self::JournalUnreadable { file, error } => format!(
                "the conversation journal {} could not be checked: {error}",
                file.display()
            ),
            Self::InboxMessage { id, kind } => {
                format!("inbox message {id} ({kind}) has not been observed")
            }
            Self::OpenEnvelope { envelope } => {
                format!("inbox delivery envelope {envelope} is still open")
            }
            Self::Job { id, state } => format!(
                "background job {id} {}",
                match state {
                    JobState::Prepared => "is about to start",
                    JobState::Running => "is running",
                    JobState::Exited => "exited but its process group is not verified clear",
                    JobState::Reaped | JobState::Lost => "has ended",
                }
            ),
            Self::Monitor { job, wait } => {
                format!("monitor {wait} still watches background job {job}")
            }
            Self::AuxiliaryChild { kind, session_id } => format!(
                "auxiliary {} child {session_id} is still running",
                match kind {
                    UsageSessionKind::Explore => "explore",
                    UsageSessionKind::Oracle => "oracle",
                    UsageSessionKind::WebSearch => "web search",
                    UsageSessionKind::Summary => "summary",
                    UsageSessionKind::Shadow => "shadow",
                    UsageSessionKind::Expert => "expert",
                }
            ),
            Self::PlanStep { plan, step, label } => format!(
                "step {step} ({label}) of plan {} is in progress and is not a wait",
                plan.as_deref().unwrap_or("(unnamed)")
            ),
            Self::UsageGuardPaused { generation } => format!(
                "the PM usage guard paused generation {generation}; `corral pm guard-resume` \
                 clears it"
            ),
        }
    }
}

/// A typed blocker with its short human sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reason {
    #[serde(flatten)]
    pub blocker: Blocker,
    pub sentence: String,
}

impl From<Blocker> for Reason {
    fn from(blocker: Blocker) -> Self {
        let sentence = blocker.sentence();
        Self { blocker, sentence }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Barrier {
    Clear,
    Blocked(Vec<Reason>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RotateOutcome {
    /// Generation `fenced` keeps its session binding; `generation` starts a fresh conversation.
    Rotated {
        pane: String,
        generation: u32,
        fenced: u32,
        previous_session: Option<String>,
        /// Blockers the owner's `--force` overrode.
        overridden: Vec<Reason>,
    },
    /// The frozen predecessor still owed something: `generation` continues its conversation
    /// through the ordinary recovery path.
    RelaunchedInstead {
        pane: String,
        generation: u32,
        fenced: u32,
        reasons: Vec<Reason>,
    },
    /// Nothing changed.
    Blocked {
        generation: u32,
        reasons: Vec<Reason>,
    },
}

#[cfg(test)]
thread_local! {
    /// Runs between the pre-check and the freeze: models input arriving in that window.
    static BEFORE_FREEZE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// The rotation pre-check of the recorded PM, read under the `pm-recovery` lease.
pub fn barrier(ctx: &Ctx) -> Result<Barrier> {
    let lease = PmRecoveryLease::acquire(ctx)?;
    let n = current_generation(&lease)?;
    let (reasons, _) = survey(ctx, n, lease.pending(ctx))?;
    Ok(if reasons.is_empty() {
        Barrier::Clear
    } else {
        Barrier::Blocked(reasons)
    })
}

/// Rotate the PM into a fresh conversation. Under the `pm-recovery` lease a blocked pre-check
/// changes nothing. Otherwise the predecessor is stopped, fenced, drained and reconciled like
/// a relaunch, the barrier is read again from that frozen evidence, and generation n+1 starts
/// a new Pi session when it is still clear, or continues the conversation when anything is
/// owed. Either way it launches in a corral-owned pane, without focus. `force` (owner shell
/// only) overrides the soft reasons, at both reads; the hard ones always win.
pub fn rotate(ctx: &Ctx, reason: &str, force: bool) -> Result<RotateOutcome> {
    ensure!(
        !force || !managed_entry(),
        "pm rotate --force needs the owner's shell, not a PM, assistant or agent"
    );
    let lease = PmRecoveryLease::acquire(ctx)?;
    let fenced = current_generation(&lease)?;
    let (reasons, previous_session) = survey(ctx, fenced, lease.pending(ctx))?;
    if blocks(&reasons, force) {
        return Ok(RotateOutcome::Blocked {
            generation: fenced,
            reasons,
        });
    }
    #[cfg(test)]
    if let Some(hook) = BEFORE_FREEZE.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let pm = lease
        .current
        .as_ref()
        .context("there is no corral-pi PM to rotate")?;
    let frozen = RefCell::new(Vec::new());
    let decide = || -> Result<bool> {
        let (reasons, _) = survey(ctx, fenced, false)?;
        let fresh = !blocks(&reasons, force);
        *frozen.borrow_mut() = reasons;
        Ok(fresh)
    };
    let decide: &dyn Fn() -> Result<bool> = &decide;
    // A live idle host is stopped first, exactly as `relaunch --replace` stops it.
    let (pane, outcome) = crate::pm::successor(ctx, pm, reason, true, Some(decide))?;
    let generation = outcome["generation"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .context("the rotation outcome names no generation")?;
    let reasons = frozen.into_inner();
    if outcome["rotated"] == true {
        return Ok(RotateOutcome::Rotated {
            pane,
            generation,
            fenced,
            previous_session,
            overridden: reasons,
        });
    }
    ctx.store.log_event(
        None,
        "pm_rotation_deferred",
        json!({
            "pane": pane, "reason": reason, "fenced": fenced, "generation": generation,
            "reasons": reasons,
        }),
    )?;
    Ok(RotateOutcome::RelaunchedInstead {
        pane,
        generation,
        fenced,
        reasons,
    })
}

fn blocks(reasons: &[Reason], force: bool) -> bool {
    reasons.iter().any(|reason| !force || reason.blocker.hard())
}

fn current_generation(lease: &PmRecoveryLease) -> Result<u32> {
    lease
        .current
        .as_ref()
        .map(|pm| pm.state.binding.generation)
        .filter(|n| *n > 0)
        .context("there is no corral-pi PM to rotate")
}

fn managed_entry() -> bool {
    crate::automation_controls::agent_entry(&crate::command::Invoker::process())
        || std::env::var("CORRAL_PM").as_deref() == Ok("1")
}

/// Every blocker of generation `n`, and the Pi session ID of the conversation a rotation
/// would retire. Read before the freeze as a pre-check and after it as the decision.
fn survey(ctx: &Ctx, n: u32, starting: bool) -> Result<(Vec<Reason>, Option<String>)> {
    let mut blockers = Vec::new();
    if starting {
        blockers.push(Blocker::SuccessorStarting { generation: n });
    }
    // The work index holds open turns, pending questions and unreaped jobs; generations (at
    // most 64) name the bound conversation and usage sessions name auxiliary children.
    let selection = pm_state::Selection::work()
        .family("generations")
        .family("usage_sessions");
    let pm =
        pm_state::load_selected(&ctx.store, &selection)?.context("no corral-pi PM is bound")?;
    let state = &pm.state;
    blockers.extend(
        state
            .turns
            .iter()
            .filter(|turn| turn.generation == n && turn.ended.is_none())
            .map(|turn| Blocker::OpenTurn {
                generation: n,
                turn: turn.seq,
            }),
    );
    blockers.extend(
        state
            .questions
            .iter()
            .filter(|question| question.recorded_pending())
            .map(|question| Blocker::BoardQuestion {
                key: question.key.clone(),
            }),
    );
    let bound = state
        .generations
        .iter()
        .filter_map(|g| g.bound.as_ref().map(|bound| (g.n, bound)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, bound)| bound);
    if let Some(bound) = bound {
        let now = ctx.store.now().as_millisecond();
        match journal::owed(&bound.session_file, &bound.session_id, n, now) {
            Ok(owed) => blockers.extend(owed),
            Err(error) => blockers.push(Blocker::JournalUnreadable {
                file: bound.session_file.clone(),
                error: format!("{error:#}"),
            }),
        }
    }
    blockers.extend(inbox_blockers(ctx)?);
    for job in state.jobs.iter().filter(|job| job.needs_reap()) {
        blockers.push(Blocker::Job {
            id: job.id.clone(),
            state: job.state,
        });
        if job.state.live() {
            blockers.extend(job.monitors.iter().map(|monitor| Blocker::Monitor {
                job: job.id.clone(),
                wait: monitor.wait.clone(),
            }));
        }
    }
    blockers.extend(
        state
            .usage_sessions
            .iter()
            .filter(|session| {
                session.generation == n
                    && matches!(
                        session.outcome,
                        Some(AuxiliaryOutcome::Prepared | AuxiliaryOutcome::Running)
                    )
            })
            .map(|session| Blocker::AuxiliaryChild {
                kind: session.kind.clone(),
                session_id: session.session_id.clone(),
            }),
    );
    if let Some(todo) = &state.todo {
        blockers.extend(
            todo.steps()
                .filter(|step| {
                    step.status == StepState::InProgress
                        && !todo.waits.iter().any(|wait| wait.step == step.id)
                })
                .map(|step| Blocker::PlanStep {
                    plan: todo.plan.clone(),
                    step: step.id.clone(),
                    label: step.label.clone(),
                }),
        );
    }
    if ctx
        .store
        .records_transaction(|| crate::pm_usage_guard::paused(ctx, n))?
    {
        blockers.push(Blocker::UsageGuardPaused { generation: n });
    }
    let session = bound.map(|bound| bound.session_id.clone());
    Ok((blockers.into_iter().map(Reason::from).collect(), session))
}

/// The PM inbox's open envelope and every message neither observed nor terminal.
fn inbox_blockers(ctx: &Ctx) -> Result<Vec<Blocker>> {
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let mut blockers = Vec::new();
    if let Some(envelope) = inbox.state()?.open_envelope {
        blockers.push(Blocker::OpenEnvelope { envelope });
    }
    for message in inbox.pending_messages()? {
        if inbox
            .receipt(message.seq, ReceiptLevel::Observed)?
            .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            blockers.push(Blocker::InboxMessage {
                id: message.id,
                kind: message.kind,
            });
        }
    }
    Ok(blockers)
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pm_rotation_tests.rs"]
mod tests;

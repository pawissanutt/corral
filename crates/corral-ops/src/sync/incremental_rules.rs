//! The step table of the incremental contract (A.1.1) as data: one [`Rule`] per gated step.
//! A rule is the union of the detectors for the inputs its step reads.
use jiff::SignedDuration;

use super::{EVERY_LONG, EVERY_SANDBOX, EVERY_SHORT, StepId};

/// `movement::QUIET_SECS` (2 s) plus one second.
const MOVEMENT_QUIET: SignedDuration = SignedDuration::from_secs(3);
/// The reader's existing attention refresh.
const EVERY_ATTENTION: SignedDuration = SignedDuration::from_secs(30);
/// The existing 15 s gauge sampling; lock `report_due` and PM retry are clock-driven.
const EVERY_SUPERVISE: SignedDuration = SignedDuration::from_secs(15);
/// Review autopilot inputs not observed: infra waits, untracked worktree edits.
const EVERY_AUTOPILOT: SignedDuration = SignedDuration::from_secs(300);

/// Tokens (A.1) whose change runs a step. Per-task rules run a task when it is in `D`, or
/// when one of their other tokens fires; there `git_repos` means the task's project repo and
/// `every` is measured per task.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rule {
    pub(super) per_task: bool,
    /// `ANY`.
    pub(super) any: bool,
    /// `T(any)`.
    pub(super) tasks: bool,
    /// `R:` areas; `a*` is a prefix.
    pub(super) areas: &'static [&'static str],
    /// `E:` kinds; `*` is any kind.
    pub(super) events: &'static [&'static str],
    /// `S`.
    pub(super) scratch: bool,
    /// `L`.
    pub(super) lifecycle: bool,
    /// `INV` of this pass or the previous one.
    pub(super) inventory: bool,
    /// `RES`.
    pub(super) resources: bool,
    /// `CFG` (also a full pass).
    pub(super) config: bool,
    /// `GIT(any project repo)`.
    pub(super) git_repos: bool,
    /// `FS(scratch/tmp/launch-exits)`.
    pub(super) launch_exits: bool,
    /// `STICKY`.
    pub(super) sticky: bool,
    /// `IN-PASS`.
    pub(super) in_pass: bool,
    /// Some open head's gate is running (result files appear asynchronously).
    pub(super) gate_running: bool,
    /// `every N`.
    pub(super) every: Option<SignedDuration>,
    /// `due` this long after the pass in which the step last acted.
    pub(super) settle: Option<SignedDuration>,
    /// `due` this long after the newest `approved` event.
    pub(super) approved: Option<SignedDuration>,
    /// `due` at the next UTC day boundary (`dispatch_policy::day`).
    pub(super) utc_day: bool,
}

impl Rule {
    /// Fires on nothing; the base of every table row.
    pub(super) const NEVER: Rule = Rule {
        per_task: false,
        any: false,
        tasks: false,
        areas: &[],
        events: &[],
        scratch: false,
        lifecycle: false,
        inventory: false,
        resources: false,
        config: false,
        git_repos: false,
        launch_exits: false,
        sticky: false,
        in_pass: false,
        gate_running: false,
        every: None,
        settle: None,
        approved: None,
        utc_day: false,
    };
}

/// Every gated step, in call order.
pub(super) const STEPS: [StepId; 40] = [
    StepId::HoldFirst,
    StepId::DesktopFirst,
    StepId::Digest,
    StepId::MigrateBacklog,
    StepId::TrackerReconcile,
    StepId::ResourcePhase,
    StepId::LessonsDue,
    StepId::PmPolicy,
    StepId::GateCollect,
    StepId::ScratchReclaim,
    StepId::LifecycleGc,
    StepId::OutputsSchedule,
    StepId::SweepInventory,
    StepId::Obligations,
    StepId::HeadLoop,
    StepId::NativeLeftover,
    StepId::OwnerDelivery,
    StepId::PmInbox,
    StepId::AgentHooksPass,
    StepId::ReviewBudget,
    StepId::PostLand,
    StepId::HandoffRecovery,
    StepId::ResumeGates,
    StepId::GateQueue,
    StepId::PlanPilot,
    StepId::Availability,
    StepId::Autopilot,
    StepId::AutopilotWorkspace,
    StepId::HoldSecond,
    StepId::Movement,
    StepId::ReacquireWoken,
    StepId::StartQueued,
    StepId::DispatchPolicy,
    StepId::Attention,
    StepId::PmEvents,
    StepId::DesktopSecond,
    StepId::SandboxSweep,
    StepId::Usage,
    StepId::Supervise,
    StepId::MaintainDatabase,
];

const HOLD: Rule = Rule {
    any: true,
    ..Rule::NEVER
};

const DESKTOP: Rule = Rule {
    tasks: true,
    config: true,
    ..Rule::NEVER
};

const PER_TASK: Rule = Rule {
    per_task: true,
    ..Rule::NEVER
};

/// The rule of `step` (A.1.1; deviations from the contract are commented).
pub(super) fn rule(step: StepId) -> Rule {
    match step {
        // 2 and 35.
        StepId::HoldFirst => HOLD,
        StepId::HoldSecond => Rule {
            in_pass: true,
            ..HOLD
        },
        // 3 and 43.
        StepId::DesktopFirst => DESKTOP,
        StepId::DesktopSecond => Rule {
            in_pass: true,
            ..DESKTOP
        },
        // 4: due = last + every, computed inside.
        StepId::Digest => Rule {
            config: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 5, 6.
        StepId::MigrateBacklog | StepId::TrackerReconcile => Rule {
            tasks: true,
            ..Rule::NEVER
        },
        // 8.
        StepId::ResourcePhase => Rule {
            resources: true,
            areas: &["resource-alert", "claims"],
            sticky: true,
            ..Rule::NEVER
        },
        // 9: the "never when all triggers are off" shortcut is left out (runs more).
        StepId::LessonsDue => Rule {
            events: &[
                "approved",
                "gate",
                "discarded",
                "escalated",
                "changes_requested",
                "lessons_distill_requested",
            ],
            config: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 11: runs before row 13, so INV is the previous pass's.
        StepId::PmPolicy => Rule {
            config: true,
            areas: &["pm*", "operating*"],
            inventory: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 16.
        StepId::GateCollect => Rule {
            tasks: true,
            gate_running: true,
            ..Rule::NEVER
        },
        // 17.
        StepId::ScratchReclaim => Rule {
            scratch: true,
            tasks: true,
            inventory: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 18: retry floor 60 s.
        StepId::LifecycleGc => Rule {
            tasks: true,
            lifecycle: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 19: the "never when config is empty" shortcut is left out; due = `next` inside.
        StepId::OutputsSchedule => Rule {
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 20.
        StepId::SweepInventory => Rule {
            inventory: true,
            tasks: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 21, 22, 23, 32: per task in D.
        // Every global row below runs after the head loop (rows 21-23). Each that reads store
        // state through `ANY`/`T(any)` also takes `IN-PASS`: a write earlier in this pass is
        // such a change, and skipping until the next pass is a wrong skip under audit.
        StepId::Obligations | StepId::HeadLoop | StepId::NativeLeftover | StepId::PlanPilot => {
            PER_TASK
        }
        // 24: recipient readiness is external.
        StepId::OwnerDelivery => Rule {
            any: true,
            inventory: true,
            in_pass: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 25: PM health is external.
        StepId::PmInbox => Rule {
            any: true,
            in_pass: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 26.
        StepId::AgentHooksPass => Rule {
            areas: &["pm*", "assistant*"],
            sticky: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 27: `review_budget` keeps its launches in area `review-launch`.
        StepId::ReviewBudget => Rule {
            tasks: true,
            inventory: true,
            in_pass: true,
            areas: &["review-budget", "review-launch"],
            ..Rule::NEVER
        },
        // 28: running hooks are judged by process liveness.
        StepId::PostLand => Rule {
            areas: &["post-land"],
            tasks: true,
            git_repos: true,
            in_pass: true,
            sticky: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 29.
        StepId::HandoffRecovery => Rule {
            areas: &["assistant-handoff*"],
            sticky: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 30: contention retries when capacity frees.
        StepId::ResumeGates => Rule {
            any: true,
            resources: true,
            in_pass: true,
            ..Rule::NEVER
        },
        // 31: the "no unfinished native heads" shortcut is left out (runs more).
        StepId::GateQueue => Rule {
            any: true,
            in_pass: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 33: deviation: `availability::advance_all` is one workspace-wide call, and its
        // selected continuations and `waiting_until` live in the `availability` record, not
        // on heads, so it is a global rule.
        StepId::Availability => Rule {
            any: true,
            inventory: true,
            in_pass: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 34: D_ap = D ∪ GIT(task's project repo) ∪ all Review tasks on R:operating*,
        // authorizations ∨ RES ∪ every 300 s per task. GIT(worktree) is part of D.
        StepId::Autopilot => Rule {
            per_task: true,
            areas: &["operating*", "authorizations"],
            resources: true,
            git_repos: true,
            every: Some(EVERY_AUTOPILOT),
            ..Rule::NEVER
        },
        // 34b.
        StepId::AutopilotWorkspace => Rule {
            any: true,
            in_pass: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 36: `reconcile` saves each active run's `after = event_tail()`, so it acts on any new
        // event (`E:*`), including events this pass's earlier steps logged (`IN-PASS`). The
        // quiet window runs from `changed_at`, the pass time of the run that wrote
        // `main-movement`, hence `settle` as well as the approval due.
        StepId::Movement => Rule {
            events: &["*"],
            areas: &["main-movement"],
            tasks: true,
            sticky: true,
            in_pass: true,
            settle: Some(MOVEMENT_QUIET),
            approved: Some(MOVEMENT_QUIET),
            ..Rule::NEVER
        },
        // 38.
        StepId::ReacquireWoken => Rule {
            tasks: true,
            in_pass: true,
            ..Rule::NEVER
        },
        // 39: harness asset files are not observed.
        StepId::StartQueued => Rule {
            tasks: true,
            areas: &["queue-refusals", "operating*"],
            config: true,
            resources: true,
            git_repos: true,
            in_pass: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
        // 40: the UTC day partition of `max_auto_promotions_per_day`.
        StepId::DispatchPolicy => Rule {
            tasks: true,
            config: true,
            resources: true,
            in_pass: true,
            utc_day: true,
            ..Rule::NEVER
        },
        // 41: output depends on `now`.
        StepId::Attention => Rule {
            any: true,
            inventory: true,
            resources: true,
            in_pass: true,
            every: Some(EVERY_ATTENTION),
            ..Rule::NEVER
        },
        // 42: a failed prompt keeps its cursor and does not write. Events logged earlier in
        // this pass are on its feed (`IN-PASS`).
        StepId::PmEvents => Rule {
            events: &["*"],
            areas: &["pm", "pm-events"],
            inventory: true,
            sticky: true,
            in_pass: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // 45.
        StepId::SandboxSweep => Rule {
            launch_exits: true,
            tasks: true,
            in_pass: true,
            every: Some(EVERY_SANDBOX),
            ..Rule::NEVER
        },
        // D1: transcripts grow outside the store.
        StepId::Usage => Rule {
            tasks: true,
            every: Some(EVERY_SHORT),
            ..Rule::NEVER
        },
        // D2.
        StepId::Supervise => Rule {
            any: true,
            areas: &["pm*"],
            every: Some(EVERY_SUPERVISE),
            ..Rule::NEVER
        },
        // D3.
        StepId::MaintainDatabase => Rule {
            any: true,
            every: Some(EVERY_LONG),
            ..Rule::NEVER
        },
    }
}

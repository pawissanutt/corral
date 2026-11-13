//! The daemon's incremental sync planner (incremental contract A.1-A.3): which gated steps and
//! which tasks one pass runs, decided from the inputs that changed since the previous pass or
//! came due. All state lives in a daemon-owned [`PassMemory`]; an empty memory plans a full pass.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::backend::ChangeSeq;
use corral_core::{Config, GateResult, TaskHead};
use corral_herdr::PaneInfo;
use jiff::{SignedDuration, Timestamp};

use super::gate::{Effect, StepGate, StepId};
use super::task_due::TaskMemo;
use crate::Ctx;
use crate::feed_delta::FeedDelta;
use crate::git_watch::{GitChanges, GitTarget, GitWatch, stat_fingerprint};
use crate::resources::ResourceState;

#[path = "incremental_rules.rs"]
mod rules;
use rules::{Rule, rule, tokens_fire};

#[path = "incremental_eval.rs"]
mod eval;
use eval::{area_matches, inventory_fingerprint, next_due, step_due, task_dirty, task_tokens};

#[path = "incremental_audit.rs"]
mod audit;
use audit::AuditProbe;

/// No later than today's measured quiet cadence (about 27 s).
pub(crate) const EVERY_SHORT: SignedDuration = SignedDuration::from_secs(20);
/// Steps whose own retry floor is at least 60 s.
pub(crate) const EVERY_LONG: SignedDuration = SignedDuration::from_secs(60);
/// `sandbox_cleanup::RECHECK`.
pub(crate) const EVERY_SANDBOX: SignedDuration = SignedDuration::from_secs(300);
/// Safety net: an input the rules miss delays a step by at most this.
pub(crate) const FULL_EVERY: SignedDuration = SignedDuration::from_secs(600);
/// Feed entries classified per pass before it becomes full.
pub(crate) const FEED_LIMIT: usize = 4096;

/// Record areas read by the per-task steps (rows 21-23, 32-34) for any task, whatever the key.
const GLOBAL_TASK_AREAS: [&str; 8] = [
    "session",
    "availability",
    "authorizations",
    "claims",
    "operating*",
    "provider-stall",
    // `sync::reconcile` reads it through `resource_escalation::holds_attention`.
    "resource-alert",
    // Review budget launches release or take shared review capacity.
    "review-launch",
];

/// Areas only global steps read (each checked against every per-task callee). A write
/// elsewhere whose area and key name no task marks every open task (conservative: an unlisted
/// area costs work, never a skipped decision). `a*` is a prefix.
const TASK_INDEPENDENT_AREAS: [&str; 11] = [
    "pm-events",
    "pm-event-wakes",
    "pm-wake-keys-*",
    "pm-usage-guard",
    "pm-supervisor",
    "agent-wait-deadlines",
    "desktop-notifications",
    "digest-schedule",
    "lessons-distill",
    "output-lifecycle",
    "queue-refusals",
];

/// PM state that per-task steps read only for tasks that open or wait on the PM (plan, plan
/// run, plan review, aggregate attempt) or hold unfinished native work (PM receipts, worker
/// hosts). A write marks only those tasks.
const PM_AREAS: [&str; 5] = ["pm", "pm-policy", "inbox/pm/*", "agent-pm*", "pm-inbox*"];

/// Cursors and state that only the step writing them reads (`storage_gc::lifecycle`'s
/// `CURSOR`, `movement`'s `RECORDS`; verified by grep, migration aside). Their writes follow
/// other changes, so they are neither `ANY` nor a task mark: they fire only rules that list
/// the area. Counting them made every task change cost a second echo pass.
const SELF_AREAS: [&str; 2] = ["lifecycle-gc", "main-movement"];

/// How the daemon asked for this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassRequest {
    Paced,
    Nudged,
    Full(FullReason),
}

/// Why a pass ran every step and every task. With timing on, each full pass counts once in
/// span `sync.full` and once in `sync.full.<reason>` ([`FullReason::span`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FullReason {
    /// Empty memory: the daemon's first pass.
    Start,
    /// An explicit refresh: a `cli` or `board` nudge.
    Owner,
    /// The `sync-full-on` switch file.
    Switch,
    Periodic,
    Config,
    Session,
    Herdr,
    FeedGap,
    ClockJump,
    Audit,
}

impl FullReason {
    /// The timing span counted once per full pass with this reason.
    pub fn span(self) -> &'static str {
        match self {
            Self::Start => "sync.full.start",
            Self::Owner => "sync.full.owner",
            Self::Switch => "sync.full.switch",
            Self::Periodic => "sync.full.periodic",
            Self::Config => "sync.full.config",
            Self::Session => "sync.full.session",
            Self::Herdr => "sync.full.herdr",
            Self::FeedGap => "sync.full.feed_gap",
            Self::ClockJump => "sync.full.clock_jump",
            Self::Audit => "sync.full.audit",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PassReport {
    pub full: Option<FullReason>,
    pub ran: usize,
    pub skipped: usize,
    /// No gated step ran (only the ALWAYS rows).
    pub quiet: bool,
    /// Row 41 ran, so the returned attention board is current.
    pub attention: bool,
    /// Earliest due time the planner knows of (task or step), strictly after the pass.
    pub next_due: Option<Timestamp>,
    /// Audit mode only: steps the rules would have skipped that wrote or returned a Change,
    /// although no input their rule lists changed during the pass and no cadence covers them.
    pub wrong_skips: Vec<StepId>,
    /// Audit mode only: such steps whose rule fires on a change made after the pass sampled
    /// its inputs; the next pass's delta (from the same sample) runs them.
    pub raced: Vec<StepId>,
    /// Audit mode only: such steps whose rule's `every` cadence (not yet due) would have run
    /// them within that bound.
    pub delayed: Vec<StepId>,
}

/// Incremental state of the daemon loop. Empty after a restart, so the first pass is full.
#[derive(Debug)]
pub struct PassMemory {
    cursor: Option<ChangeSeq>,
    config: Option<Config>,
    inventory: Option<u64>,
    inventory_prev: bool,
    git: GitWatch,
    worktrees: BTreeMap<String, PathBuf>,
    branches: BTreeMap<String, BTreeSet<String>>,
    launch_exits: Option<(bool, u64, i128, u64)>,
    resources: Option<(bool, bool, Option<String>)>,
    herdr: Option<bool>,
    gate_running: bool,
    last_run: BTreeMap<StepId, Timestamp>,
    acted_at: BTreeMap<StepId, Timestamp>,
    sticky: BTreeSet<StepId>,
    task_sticky: BTreeSet<String>,
    /// Tasks with unfinished native work, listed when a [`PM_AREAS`] write needs them.
    native: BTreeSet<String>,
    tasks: BTreeMap<String, TaskMemo>,
    task_run: BTreeMap<(StepId, String), Timestamp>,
    approved_at: Option<Timestamp>,
    last_full: Option<Timestamp>,
    last_now: Option<Timestamp>,
    extra_any: bool,
    extra_tasks: bool,
    audit: bool,
    rules: fn(StepId) -> Rule,
}

impl Default for PassMemory {
    fn default() -> Self {
        Self {
            cursor: None,
            config: None,
            inventory: None,
            inventory_prev: false,
            git: GitWatch::default(),
            worktrees: BTreeMap::new(),
            branches: BTreeMap::new(),
            launch_exits: None,
            resources: None,
            herdr: None,
            gate_running: false,
            last_run: BTreeMap::new(),
            acted_at: BTreeMap::new(),
            sticky: BTreeSet::new(),
            task_sticky: BTreeSet::new(),
            native: BTreeSet::new(),
            tasks: BTreeMap::new(),
            task_run: BTreeMap::new(),
            approved_at: None,
            last_full: None,
            last_now: None,
            extra_any: false,
            extra_tasks: false,
            audit: false,
            rules: rule,
        }
    }
}

impl PassMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Audit mode: every pass is full, and steps the rules would have skipped but that acted
    /// are reported as wrong skips.
    pub fn set_audit(&mut self, on: bool) {
        self.audit = on;
    }

    /// Daemon extras D1-D3 use the same rules after the pass, on the tokens the pass saw.
    pub fn extra_due(&mut self, step: StepId, report: &PassReport, now: Timestamp) -> bool {
        let rule = (self.rules)(step);
        let due = report.full.is_some()
            || (rule.any && self.extra_any)
            || (rule.tasks && self.extra_tasks)
            || step_due(self, step, &rule).is_some_and(|at| at <= now);
        if due {
            self.last_run.insert(step, now);
        }
        due
    }

    /// Classifies the feed since the previous pass and stats git and the launch-exit owners.
    pub(crate) fn begin(
        &mut self,
        ctx: &Ctx,
        request: PassRequest,
        now: Timestamp,
    ) -> Result<Planner<'_>> {
        let _plan = corral_core::timing::span("sync.plan");
        let feed = {
            let _span = corral_core::timing::span("sync.feed_delta");
            let view = ctx.store.view().context("reading the change feed")?;
            match self.cursor {
                Some(cursor) => FeedDelta::since(view.as_ref(), cursor, FEED_LIMIT),
                None => FeedDelta {
                    through: view.seq(),
                    ..FeedDelta::default()
                },
            }
        };
        let git = {
            let _span = corral_core::timing::span("sync.git_watch");
            let (targets, unresolved) = self.git_targets(ctx);
            let mut git = self.git.refresh(&targets);
            git.projects.extend(unresolved);
            git
        };
        let launch_exits = ctx
            .store
            .scratch_path("tmp")
            .ok()
            .map(|tmp| stat_fingerprint(&tmp.join("launch-exits")));
        let launch_changed = launch_exits.is_none() || launch_exits != self.launch_exits;
        self.launch_exits = launch_exits;
        let mut planner = self.plan(request, now, &ctx.store.config, feed, git, launch_changed);
        if planner.audit {
            planner.probe = Some(AuditProbe::new(&ctx.store, planner.tokens.feed.through));
        }
        if planner.tokens.pm {
            planner.memory.native = native_ids(ctx)?;
        }
        Ok(planner)
    }

    /// The pure decision core of [`Self::begin`]: full-pass reasons and the pass tokens.
    pub(super) fn plan(
        &mut self,
        request: PassRequest,
        now: Timestamp,
        config: &Config,
        feed: FeedDelta,
        git: GitChanges,
        launch_exits: bool,
    ) -> Planner<'_> {
        let config_changed = self.config.as_ref().is_some_and(|c| c != config);
        if self.config.as_ref() != Some(config) {
            self.config = Some(config.clone());
        }
        let clock_jump = self
            .last_now
            .is_some_and(|last| now < last || now.duration_since(last) > FULL_EVERY);
        let periodic = self
            .last_full
            .is_none_or(|last| now.duration_since(last) >= FULL_EVERY);
        let full = match request {
            PassRequest::Full(reason) => Some(reason),
            PassRequest::Paced | PassRequest::Nudged => None,
        }
        .or_else(|| self.cursor.is_none().then_some(FullReason::Start))
        .or_else(|| feed.overflow.then_some(FullReason::FeedGap))
        .or_else(|| clock_jump.then_some(FullReason::ClockJump))
        .or_else(|| config_changed.then_some(FullReason::Config))
        .or_else(|| {
            feed.areas
                .contains_key("session")
                .then_some(FullReason::Session)
        })
        .or_else(|| periodic.then_some(FullReason::Periodic))
        .or_else(|| self.audit.then_some(FullReason::Audit));
        self.cursor = Some(feed.through);
        self.note_approved(&feed);
        let tokens = Tokens::new(feed, git, launch_exits, config_changed);
        Planner {
            memory: self,
            now,
            full,
            full_at_start: full.is_some(),
            audit: full == Some(FullReason::Audit),
            tokens,
            dirty: BTreeMap::new(),
            fresh: BTreeSet::new(),
            heads: None,
            seen_tasks: BTreeSet::new(),
            evaluated: BTreeSet::new(),
            acted_tasks: BTreeSet::new(),
            would_skip: BTreeMap::new(),
            probe: None,
            report: PassReport {
                full,
                ..PassReport::default()
            },
        }
    }

    fn note_approved(&mut self, feed: &FeedDelta) {
        if let Some(at) = feed.newest_event_at.get("approved") {
            self.approved_at = Some(self.approved_at.map_or(*at, |old| old.max(*at)));
        }
    }

    /// Worktrees of the open tasks' current runs (as of the last head list) and every
    /// configured project repo with `main`/base plus the bases its runs target.
    fn git_targets(&self, ctx: &Ctx) -> (Vec<GitTarget>, BTreeSet<String>) {
        let mut targets: Vec<GitTarget> = self
            .worktrees
            .iter()
            .map(|(task, path)| GitTarget::Worktree {
                task: task.clone(),
                path: path.clone(),
            })
            .collect();
        let mut unresolved = BTreeSet::new();
        for (name, project) in &ctx.store.config.projects {
            let Ok(path) = ctx.store.project_path(name) else {
                // Unwatchable: counts as changed, as for git_watch's own unwatchable targets.
                unresolved.insert(name.clone());
                continue;
            };
            let mut branches = self.branches.get(name).cloned().unwrap_or_default();
            branches.insert(project.base.clone().unwrap_or_else(|| "main".into()));
            targets.push(GitTarget::Repo {
                project: name.clone(),
                path,
                branches: branches.into_iter().collect(),
            });
        }
        (targets, unresolved)
    }
}

/// The inputs of one pass iteration (A.1 tokens).
#[derive(Debug, Default)]
struct Tokens {
    feed: FeedDelta,
    git: GitChanges,
    launch_exits: bool,
    config_changed: bool,
    inventory: bool,
    resources: bool,
    in_pass: bool,
    /// `ANY`: a non-lease change outside [`SELF_AREAS`], or an incomplete classification.
    any: bool,
    /// A [`PM_AREAS`] write.
    pm: bool,
    /// Task numbers named by a changed record key (`T-<n>` token).
    keyed: BTreeSet<u64>,
    /// A record area read for every task changed.
    all_tasks: bool,
    /// Pane ids of this pass's inventory, once observed.
    panes: Option<BTreeSet<String>>,
}

impl Tokens {
    fn new(feed: FeedDelta, git: GitChanges, launch_exits: bool, config_changed: bool) -> Self {
        let mut keyed = BTreeSet::new();
        let mut all_tasks = false;
        let mut pm = false;
        for (area, keys) in &feed.areas {
            if SELF_AREAS.contains(&area.as_str()) {
                continue;
            }
            if PM_AREAS.iter().any(|p| area_matches(p, area)) {
                pm = true;
                keyed.extend(keys.iter().flat_map(|key| task_tokens(key)));
                continue;
            }
            let global = GLOBAL_TASK_AREAS.iter().any(|p| area_matches(p, area));
            let independent = TASK_INDEPENDENT_AREAS.iter().any(|p| area_matches(p, area));
            for key in keys {
                let mut named = false;
                for task in task_tokens(key).chain(task_tokens(area)) {
                    keyed.insert(task);
                    named = true;
                }
                all_tasks |= global || (!named && !independent);
            }
        }
        let any = feed.overflow
            || !feed.tasks.is_empty()
            || !feed.task_ids.is_empty()
            || feed
                .areas
                .keys()
                .any(|area| !SELF_AREAS.contains(&area.as_str()))
            || !feed.event_kinds.is_empty()
            || feed.scratch
            || feed.lifecycle
            || feed.aggregates
            || feed.blobs;
        Self {
            feed,
            git,
            launch_exits,
            config_changed,
            keyed,
            all_tasks,
            any,
            pm,
            ..Self::default()
        }
    }

    fn tasks_changed(&self) -> bool {
        !self.feed.tasks.is_empty() || !self.feed.task_ids.is_empty()
    }
}

/// Decides one daemon pass; borrows the loop's [`PassMemory`].
pub(crate) struct Planner<'a> {
    memory: &'a mut PassMemory,
    now: Timestamp,
    full: Option<FullReason>,
    full_at_start: bool,
    audit: bool,
    tokens: Tokens,
    /// Per-task dirty decisions of this iteration.
    dirty: BTreeMap<String, bool>,
    /// Task memos refreshed from a head this pass.
    fresh: BTreeSet<String>,
    /// Open task ids of row 15, once observed.
    heads: Option<BTreeSet<String>>,
    seen_tasks: BTreeSet<String>,
    /// Tasks evaluated in this iteration.
    evaluated: BTreeSet<String>,
    acted_tasks: BTreeSet<String>,
    /// Audit mode: steps and tasks the rules would have skipped, and why they might act.
    would_skip: BTreeMap<(StepId, Option<String>), Skip>,
    probe: Option<AuditProbe>,
    report: PassReport,
}

impl Planner<'_> {
    /// A `hot_passes` repeat: classifies only the feed written since the iteration started.
    pub(crate) fn next_iteration(&mut self, ctx: &Ctx) -> Result<()> {
        let view = ctx.store.view().context("reading the change feed")?;
        let after = self.memory.cursor.unwrap_or_default();
        let feed = FeedDelta::since(view.as_ref(), after, FEED_LIMIT);
        drop(view);
        if self.full.is_none() {
            self.full = feed.overflow.then_some(FullReason::FeedGap).or_else(|| {
                feed.areas
                    .contains_key("session")
                    .then_some(FullReason::Session)
            });
        }
        self.memory.cursor = Some(feed.through);
        if let Some(probe) = self.probe.as_mut() {
            probe.restart(feed.through);
        }
        self.memory.note_approved(&feed);
        self.memory.inventory_prev = self.tokens.inventory;
        self.settle_tasks();
        let panes = self.tokens.panes.take();
        self.tokens = Tokens::new(feed, GitChanges::default(), false, false);
        self.tokens.panes = panes;
        if self.tokens.pm {
            self.memory.native = native_ids(ctx)?;
        }
        self.dirty.clear();
        Ok(())
    }

    pub(crate) fn finish(mut self, now: Timestamp) -> PassReport {
        self.settle_tasks();
        let memory = &mut *self.memory;
        if let Some(heads) = &self.heads {
            memory
                .tasks
                .retain(|id, _| heads.contains(id) || self.seen_tasks.contains(id));
        }
        let tasks = &memory.tasks;
        memory.task_run.retain(|(_, id), _| tasks.contains_key(id));
        memory.task_sticky.retain(|id| tasks.contains_key(id));
        memory.inventory_prev = self.tokens.inventory;
        memory.extra_any = self.tokens.any || self.tokens.in_pass;
        memory.extra_tasks = self.tokens.tasks_changed() || self.tokens.in_pass;
        if self.full_at_start {
            memory.last_full = Some(self.now);
        }
        memory.last_now = Some(self.now);
        self.report.full = self.full;
        if let Some(reason) = self.full {
            drop(corral_core::timing::span("sync.full"));
            drop(corral_core::timing::span(reason.span()));
        }
        if self.audit {
            // A positive record per audited pass: zero wrong skips is then distinguishable
            // from audit never running.
            drop(corral_core::timing::span("sync.audit.pass"));
        }
        self.report.quiet = self.report.ran == 0;
        if self.report.quiet {
            drop(corral_core::timing::span("sync.quiet"));
        }
        self.report.next_due = next_due(memory, now);
        self.report
    }

    /// Tasks that acted stay dirty for one more run; unevaluated ones keep their mark.
    fn settle_tasks(&mut self) {
        let evaluated = std::mem::take(&mut self.evaluated);
        self.memory.task_sticky.retain(|id| !evaluated.contains(id));
        self.memory
            .task_sticky
            .extend(std::mem::take(&mut self.acted_tasks));
    }

    fn fires(&self, step: StepId, rule: &Rule) -> bool {
        self.tokens_fire(step, rule)
            || (rule.git_repos && !self.tokens.git.projects.is_empty())
            || step_due(&*self.memory, step, rule).is_some_and(|at| at <= self.now)
    }

    /// Every token of `rule` except the git and clock ones, which per-task rules scope.
    fn tokens_fire(&self, step: StepId, rule: &Rule) -> bool {
        tokens_fire(&self.tokens, &*self.memory, step, rule)
    }

    /// Audit: whether `rule` fires on what changed after the pass sampled its inputs.
    fn raced(&mut self, step: StepId, rule: &Rule, head: Option<&TaskHead>) -> bool {
        let Some(probe) = self.probe.as_mut() else {
            return false;
        };
        let memory = &*self.memory;
        let Some(tokens) = probe.since_sample(&self.tokens, memory.config.as_ref()) else {
            return false;
        };
        tokens_fire(tokens, memory, step, rule)
            || head.is_some_and(|head| {
                rule.per_task
                    && memory
                        .tasks
                        .get(&head.id)
                        .is_some_and(|memo| task_dirty(tokens, memory, memo, &head.id, self.now))
            })
    }

    /// Audit: why a step or task the rules skip may still act. A listed input changed after
    /// the sample (raced), or a cadence of the rule would run it within its bound (delayed).
    fn classify(&mut self, step: StepId, rule: &Rule, head: Option<&TaskHead>) -> Skip {
        if self.raced(step, rule, head) {
            return Skip::Raced;
        }
        let task_every = head
            .filter(|_| rule.per_task)
            .and_then(|head| self.memory.tasks.get(&head.id))
            .and_then(|memo| memo.due.every);
        match [rule.every, task_every].into_iter().flatten().min() {
            Some(every) => Skip::Delayed(every),
            None => Skip::Unexplained,
        }
    }

    fn task_fires(&mut self, step: StepId, rule: &Rule, head: &TaskHead) -> bool {
        // A task an earlier step of this pass wrote or acted on is input to the later ones.
        if self.acted_tasks.contains(&head.id) || self.dirty(head) || self.tokens_fire(step, rule) {
            return true;
        }
        let git = &self.tokens.git;
        let repo = rule.git_repos
            && match head.project.as_deref() {
                Some(project) => git.projects.contains(project),
                None => !git.projects.is_empty(),
            };
        let key = (step, head.id.clone());
        repo || rule.every.is_some_and(|every| {
            self.memory
                .task_run
                .get(&key)
                .is_none_or(|at| self.now.duration_since(*at) >= every)
        })
    }

    /// The task is in `D` (A.1.2).
    fn dirty(&mut self, head: &TaskHead) -> bool {
        if let Some(dirty) = self.dirty.get(&head.id) {
            return *dirty;
        }
        self.refresh_memo(head);
        let dirty =
            self.memory.tasks.get(&head.id).is_none_or(|memo| {
                task_dirty(&self.tokens, &*self.memory, memo, &head.id, self.now)
            });
        self.dirty.insert(head.id.clone(), dirty);
        dirty
    }

    /// Recomputes a task's memo from `head` once per pass, keeping its last run.
    fn refresh_memo(&mut self, head: &TaskHead) {
        if self.fresh.contains(&head.id) {
            return;
        }
        let memory = &mut *self.memory;
        let Some(config) = memory.config.as_ref() else {
            return;
        };
        let last_run = memory.tasks.get(&head.id).and_then(|memo| memo.last_run);
        memory
            .tasks
            .insert(head.id.clone(), TaskMemo::of(head, config, last_run));
        self.fresh.insert(head.id.clone());
    }

    fn record(
        &mut self,
        step: StepId,
        task: Option<&str>,
        decision: bool,
        run: bool,
        skip: Option<Skip>,
    ) {
        if run {
            self.report.ran += 1;
        } else {
            self.report.skipped += 1;
            drop(corral_core::timing::span("sync.skip"));
        }
        if self.audit {
            drop(corral_core::timing::span("sync.audit.step"));
            if let Some(skip) = skip.filter(|_| !decision) {
                drop(corral_core::timing::span("sync.audit.would_skip"));
                self.would_skip
                    .insert((step, task.map(str::to_owned)), skip);
            }
        }
    }

    /// Every task the step wrote runs the later per-task steps of this pass, and once more on
    /// the next pass (task `STICKY`).
    fn account(&mut self, step: StepId, task: Option<&str>, effect: Effect) -> bool {
        let acted = effect.wrote || effect.changes > 0;
        let key = (step, task.map(str::to_owned));
        if acted && let Some(skip) = self.would_skip.get(&key).copied() {
            let task = task.map(|t| format!(" task {t}")).unwrap_or_default();
            let head =
                format!("corrald sync audit: {step:?}{task} would have been skipped but acted");
            match skip {
                Skip::Raced => {
                    eprintln!("{head}; raced: an input its rule lists changed during the pass");
                    drop(corral_core::timing::span("sync.audit.raced"));
                    self.report.raced.push(step);
                }
                Skip::Delayed(every) => {
                    let secs = every.as_secs();
                    eprintln!("{head}; delayed: its every {secs}s cadence would have run it");
                    drop(corral_core::timing::span("sync.audit.delayed"));
                    self.report.delayed.push(step);
                }
                Skip::Unexplained => {
                    eprintln!("{head}");
                    drop(corral_core::timing::span("sync.audit.wrong_skip"));
                    self.report.wrong_skips.push(step);
                }
            }
        }
        self.tokens.in_pass |= acted;
        self.acted_tasks.extend(effect.tasks);
        acted
    }
}

impl StepGate for Planner<'_> {
    fn run(&mut self, step: StepId) -> bool {
        let rule = (self.memory.rules)(step);
        if rule.per_task {
            // The per-task decision is made by `task`.
            return true;
        }
        let decision = (self.full.is_none() || self.audit) && self.fires(step, &rule);
        let run = self.full.is_some() || decision;
        let skip = (self.audit && !decision).then(|| self.classify(step, &rule, None));
        self.record(step, None, decision, run, skip);
        if run {
            // Audit keeps the incremental schedule: only a firing rule restarts its cadence.
            if decision || !self.audit {
                self.memory.last_run.insert(step, self.now);
            }
            self.report.attention |= step == StepId::Attention;
        }
        run
    }

    fn task(&mut self, step: StepId, head: &TaskHead) -> bool {
        let rule = (self.memory.rules)(step);
        self.seen_tasks.insert(head.id.clone());
        self.evaluated.insert(head.id.clone());
        let decision = (self.full.is_none() || self.audit)
            && if rule.per_task {
                self.task_fires(step, &rule, head)
            } else {
                self.fires(step, &rule)
            };
        let run = self.full.is_some() || decision;
        let skip = (self.audit && !decision).then(|| self.classify(step, &rule, Some(head)));
        self.record(step, Some(&head.id), decision, run, skip);
        if run {
            self.refresh_memo(head);
            if decision || !self.audit {
                if let Some(memo) = self.memory.tasks.get_mut(&head.id) {
                    memo.last_run = Some(self.now);
                }
                self.memory
                    .task_run
                    .insert((step, head.id.clone()), self.now);
            }
        }
        run
    }

    fn observe_inventory(&mut self, panes: &[PaneInfo]) {
        let fingerprint = inventory_fingerprint(panes);
        self.tokens.inventory |= self.memory.inventory != Some(fingerprint);
        self.memory.inventory = Some(fingerprint);
        self.tokens.panes = Some(panes.iter().map(|p| p.pane_id.clone()).collect());
    }

    fn observe_resources(&mut self, state: &ResourceState) {
        let key = (state.held, state.severe, state.reason.clone());
        self.tokens.resources |= self.memory.resources.as_ref() != Some(&key);
        self.memory.resources = Some(key);
    }

    fn observe_config(&mut self, config: &Config) {
        if self.memory.config.as_ref() == Some(config) {
            return;
        }
        if self.memory.config.is_some() {
            // Edited after the planner's own sample: the rest of the pass is full.
            self.tokens.config_changed = true;
            if self.full.is_none() {
                self.full = Some(FullReason::Config);
            }
        }
        self.memory.config = Some(config.clone());
    }

    fn observe_herdr(&mut self, available: bool) {
        if self.memory.herdr.is_some_and(|before| before != available) && self.full.is_none() {
            self.full = Some(FullReason::Herdr);
        }
        self.memory.herdr = Some(available);
    }

    fn observe_heads(&mut self, heads: &[TaskHead]) {
        let mut worktrees = BTreeMap::new();
        let mut branches: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut gate_running = false;
        for head in heads {
            self.refresh_memo(head);
            if let Some(run) = head.current_run.as_deref() {
                if !run.worktree.as_os_str().is_empty() {
                    if self.memory.worktrees.get(&head.id) != Some(&run.worktree) {
                        // Not yet stat-watched: treat as moved, as GitWatch does a new target.
                        self.tokens.git.tasks.insert(head.id.clone());
                    }
                    worktrees.insert(head.id.clone(), run.worktree.clone());
                }
                if let Some(project) = &head.project {
                    branches
                        .entry(project.clone())
                        .or_default()
                        .insert(run.base.clone());
                }
                gate_running |= run
                    .gate
                    .as_ref()
                    .is_some_and(|g| g.result == GateResult::Running);
            }
            gate_running |= head
                .last_aggregate_attempt
                .as_deref()
                .and_then(|attempt| attempt.gate.as_ref())
                .is_some_and(|g| g.result == GateResult::Running);
        }
        let memory = &mut *self.memory;
        memory.worktrees = worktrees;
        memory.branches = branches;
        memory.gate_running = gate_running;
        self.heads = Some(heads.iter().map(|h| h.id.clone()).collect());
    }

    fn measures(&self) -> bool {
        true
    }

    fn finished(&mut self, step: StepId, effect: Effect) {
        if self.account(step, None, effect) {
            self.memory.sticky.insert(step);
            self.memory.acted_at.insert(step, self.now);
        } else {
            self.memory.sticky.remove(&step);
        }
    }

    fn finished_task(&mut self, step: StepId, task: &str, effect: Effect) {
        if self.account(step, Some(task), effect) {
            self.acted_tasks.insert(task.to_owned());
        }
    }
}

/// Why audit found a skipped step or task acting; see [`Planner::classify`].
#[derive(Debug, Clone, Copy)]
enum Skip {
    Raced,
    Delayed(SignedDuration),
    Unexplained,
}

fn native_ids(ctx: &Ctx) -> Result<BTreeSet<String>> {
    Ok(ctx
        .store
        .unfinished_native_heads()
        .context("listing tasks with native work")?
        .into_iter()
        .map(|head| head.task.id)
        .collect())
}

#[cfg(test)]
#[path = "incremental_tests.rs"]
mod tests;

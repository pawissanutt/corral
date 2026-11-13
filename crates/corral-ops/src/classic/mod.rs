//! The classic board frame: everything the classic board renders that the store derives. The
//! daemon computes and serves it; a board without a daemon computes it with the same code.
//!
//! [`ClassicCache::refresh`] classifies the change feed since its last refresh and recomputes
//! only the parts whose inputs changed or came due (contract B.6.1); `rev` moves only when a
//! part's content changed. No part precomputes age text: only `attention` and `resources`
//! depend on the clock, and both declare dues ([`ClassicCache::next_due`]).
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use corral_core::backend::{ChangeSeq, ReadView};
use corral_core::{PlanDeliveryTarget, ResourceSnapshot, Status, Task, TaskHead};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::attention::AttentionBoard;
use crate::feed_delta::FeedDelta;
use crate::git_watch::{GitChanges, GitTarget, GitWatch, stat_fingerprint};

mod parts;
mod plan;
#[cfg(test)]
mod tests;

/// The document of a `ReadKind::ClassicFrame` read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassicFrame {
    pub rev: u64,
    pub seq: u64,
    pub as_of: jiff::Timestamp,
    pub attention: Result<crate::attention::AttentionBoard, String>,
    pub operating: String,
    /// Task-number order.
    pub heads: Vec<corral_core::HeadWire>,
    /// The latest Diff artifact of each task's current run.
    pub diffs: BTreeMap<String, corral_core::Artifact>,
    pub obligations: BTreeMap<String, corral_core::RunObligations>,
    pub status_counts: Vec<corral_core::backend::StatusCount>,
    pub slot_usage: crate::SlotUsage,
    pub done_ids: BTreeSet<String>,
    pub pins: Vec<ClassicPin>,
    pub finishes: BTreeMap<String, crate::Finish>,
    pub unreviewed: BTreeMap<String, usize>,
    pub plan_progress: BTreeMap<String, Result<crate::PlanProgress, String>>,
    pub resources: ResourceGauges,
    pub waits: BTreeMap<String, String>,
    pub stages: BTreeMap<String, String>,
    pub automation_blocks: BTreeMap<String, String>,
    pub plan_review_waits: BTreeMap<String, String>,
}

/// A pin and what it resolved to when the frame was computed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassicPin {
    pub pin: corral_core::Pin,
    pub target: corral_core::PinTarget,
}

/// The board's resource line: a reason it is unavailable, or one gauge per resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceGauges {
    pub reason: Option<String>,
    pub gauges: Vec<Gauge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gauge {
    pub text: String,
    pub level: GaugeLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GaugeLevel {
    Good,
    Warn,
    Bad,
}

pub struct ClassicInputs {
    pub now: jiff::Timestamp,
    /// The caller's current attention (a pass's board, or Err from unavailable diagnostics);
    /// None makes the cache compute it with `board_hot` when the attention part is dirty.
    pub attention: Option<Result<crate::attention::AttentionBoard, String>>,
    /// Recompute every part (explicit refresh, periodic full refresh).
    pub force: bool,
}

/// Feed entries classified per refresh before every part is recomputed.
const FEED_LIMIT: usize = 4096;
/// Every part is recomputed this often: covers inputs no rule observes (templates, env).
const FULL_EVERY: SignedDuration = SignedDuration::from_secs(600);
/// `board_hot` depends on `now`; the reader's existing refresh interval.
const ATTENTION_EVERY: SignedDuration = SignedDuration::from_secs(30);
const GAUGES_EVERY: SignedDuration = SignedDuration::from_secs(15);
/// Gate progress files change without a store write while a gate holds a slot.
const STAGES_EVERY: SignedDuration = SignedDuration::from_secs(5);
/// Record areas (prefix match) of operating policy and authorizations: automation,
/// plan-review and admission text read them.
const POLICY_AREAS: [&str; 2] = ["operating", "authorizations"];
/// Record areas (prefix match) the dispatch queue's waiting reasons also read.
const QUEUE_AREAS: [&str; 2] = ["queue", "resource"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Rows,
    Meta,
    Plan,
    Resources,
    Stages,
    Automation,
    PlanReview,
    Attention,
}

#[derive(Debug)]
struct Resources {
    /// The sample's gauges, without the dispatch queue's error gauge.
    gauges: ResourceGauges,
    admission: parts::Admission,
    waits: BTreeMap<String, String>,
    queue_error: Option<Gauge>,
}

#[derive(Debug)]
struct Attention {
    attention: Result<AttentionBoard, String>,
    operating: String,
}

/// The last computed frame and what it was computed from.
#[derive(Debug, Default)]
pub struct ClassicCache {
    frame: Option<Arc<ClassicFrame>>,
    rev: u64,
    checked_through: Option<ChangeSeq>,
    git: GitWatch,
    pins: Option<(bool, u64, i128, u64)>,
    config: Option<corral_core::Config>,
    last_full: Option<Timestamp>,
    digests: BTreeMap<Part, u64>,
    /// Rows in task-number order, and the same rows by id.
    order: Vec<Arc<parts::Row>>,
    rows: BTreeMap<String, Arc<parts::Row>>,
    meta: Option<parts::Meta>,
    trackers: BTreeMap<String, Arc<plan::Tracker>>,
    resources: Option<Resources>,
    gauges_at: Option<Timestamp>,
    stages: BTreeMap<String, String>,
    stages_at: Option<Timestamp>,
    /// A gate held a slot when stages were last computed.
    gating: bool,
    automation: BTreeMap<String, String>,
    plan_review: BTreeMap<String, String>,
    attention: Option<Attention>,
    attention_at: Option<Timestamp>,
}

/// Decisions made before the pinned read.
struct Plan<'a> {
    now: Timestamp,
    full: bool,
    pins_changed: bool,
    git: &'a GitChanges,
    stages_due: bool,
    attention_due: bool,
    /// The caller supplied attention, so the cache does not compute it.
    supplied: bool,
}

/// Everything one pinned read recomputed; parts left None keep their cached value.
struct Computed {
    seq: ChangeSeq,
    full: bool,
    listing: Option<parts::Listing>,
    trackers: BTreeMap<String, Arc<plan::Tracker>>,
    resources: Resources,
    /// Stages, whether a gate holds a slot, and whether every gating task was re-read.
    stages: Option<(BTreeMap<String, String>, bool, bool)>,
    automation: Option<BTreeMap<String, String>>,
    plan_review: Option<BTreeMap<String, String>>,
    attention: Option<Result<AttentionBoard, String>>,
}

impl ClassicCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The last computed frame; None before the first refresh.
    pub fn current(&self) -> Option<Arc<ClassicFrame>> {
        self.frame.clone()
    }

    /// Earliest clock-driven part refresh (attention 30 s, gauges 15 s, running-gate stages 5 s).
    pub fn next_due(&self) -> Option<jiff::Timestamp> {
        self.frame.as_ref()?;
        let after = |at: Option<Timestamp>, every: SignedDuration| {
            at.and_then(|at| at.checked_add(every).ok())
        };
        [
            after(self.attention_at, ATTENTION_EVERY),
            after(self.gauges_at, GAUGES_EVERY),
            self.gating
                .then(|| after(self.stages_at, STAGES_EVERY))
                .flatten(),
            after(self.last_full, FULL_EVERY),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Recomputes dirty parts only. `Ok(None)` when no part's content changed.
    pub fn refresh(
        &mut self,
        ctx: &Ctx,
        inputs: ClassicInputs,
    ) -> Result<Option<Arc<ClassicFrame>>> {
        let result = self.refresh_parts(ctx, inputs);
        if result.is_err() {
            // Git fingerprints may have advanced past inputs that were never applied.
            self.last_full = None;
        }
        result
    }

    fn refresh_parts(
        &mut self,
        ctx: &Ctx,
        inputs: ClassicInputs,
    ) -> Result<Option<Arc<ClassicFrame>>> {
        let now = inputs.now;
        let due = |at: Option<Timestamp>, every: SignedDuration| {
            at.is_none_or(|at| now < at || now.duration_since(at) >= every)
        };
        let config_changed = self.config.as_ref() != Some(&ctx.store.config);
        let pins = stat_fingerprint(&ctx.store.dir().join("pins.toml"));
        let pins_changed = self.pins != Some(pins);
        let targets = self.git_targets(ctx);
        let git = self.git.refresh(&targets);
        let full = inputs.force
            || self.frame.is_none()
            || config_changed
            || due(self.last_full, FULL_EVERY);
        let plan = Plan {
            now,
            full,
            pins_changed,
            git: &git,
            stages_due: self.gating && due(self.stages_at, STAGES_EVERY),
            attention_due: due(self.attention_at, ATTENTION_EVERY),
            supplied: inputs.attention.is_some(),
        };
        let any = match self.checked_through {
            Some(cursor) => FeedDelta::since(&*ctx.store.view()?, cursor, FEED_LIMIT).any(),
            None => true,
        };
        let work = full
            || any
            || pins_changed
            || !git.tasks.is_empty()
            || !git.projects.is_empty()
            || plan.stages_due
            || plan.attention_due
            || plan.supplied
            || due(self.gauges_at, GAUGES_EVERY);
        if !work {
            return Ok(None);
        }
        if any || full {
            // Preparation may wait on derivation locks or do substantial filesystem work.
            let _span = corral_core::timing::span("classic.prepare");
            crate::lesson_rows::prepare_coordination(ctx)?;
        }
        let _hot = corral_core::work::HotScope::enter();
        // Gauges, admission and attention share one host observation.
        let sampled = sampled(ctx);
        let mut computed = sampled.read_step(|read| self.compute(read, &plan))?;
        let attention = match inputs.attention {
            Some(attention) => Some(attention),
            None => computed.attention.take(),
        };
        let attention = attention.map(|attention| {
            let _span = corral_core::timing::span("classic.attention");
            let operating = operating(&sampled, &attention);
            Attention {
                attention,
                operating,
            }
        });
        if config_changed {
            self.config = Some(ctx.store.config.clone());
        }
        self.pins = Some(pins);
        self.apply(computed, attention, now)
    }

    /// Recompute the dirty parts on one pinned read. Reads `self` only, so an expired read
    /// can be retried.
    fn compute(&self, read: &Ctx, plan: &Plan<'_>) -> Result<Computed> {
        let delta = {
            let view: Box<dyn ReadView + Send> = read.store.view()?;
            match self.checked_through {
                Some(cursor) => FeedDelta::since(&*view, cursor, FEED_LIMIT),
                None => FeedDelta {
                    overflow: true,
                    through: view.seq(),
                    ..FeedDelta::default()
                },
            }
        };
        let full = plan.full || delta.overflow;
        let touched: BTreeSet<String> = delta
            .tasks
            .iter()
            .map(|num| format!("T-{:03}", num.0))
            .chain(delta.task_ids.iter().cloned())
            .collect();
        let area = |prefixes: &[&str]| {
            delta
                .areas
                .keys()
                .any(|area| prefixes.iter().any(|prefix| area.starts_with(prefix)))
        };
        let policy = area(&POLICY_AREAS);
        let assistant_pins = self
            .meta
            .as_ref()
            .is_some_and(|meta| meta.pins.iter().any(|pin| pin.pin.target.starts_with("A-")))
            && area(&["assistant"]);
        let listing = if full
            || !touched.is_empty()
            || delta.aggregates
            || plan.pins_changed
            || assistant_pins
        {
            Some(parts::listing(
                read,
                &parts::Reuse {
                    previous: &self.rows,
                    touched: &touched,
                    aggregates: delta.aggregates,
                    full,
                },
            )?)
        } else {
            None
        };
        let no_change = BTreeSet::new();
        let (rows, changed) = match &listing {
            Some(listing) => (listing.rows.as_slice(), &listing.changed),
            None => (self.order.as_slice(), &no_change),
        };
        let tasks: Vec<&Task> = rows.iter().map(|row| &row.shell).collect();
        let slot_usage = listing
            .as_ref()
            .map(|listing| &listing.meta.slot_usage)
            .or(self.meta.as_ref().map(|meta| &meta.slot_usage));

        let sample = parts::sample(read);
        let previous = self.resources.as_ref();
        let admission = policy
            || previous.is_none_or(|resources| resources.admission != sample.admission)
            || self.meta.as_ref().map(|meta| &meta.slot_usage) != slot_usage;
        let held = sample.admission.as_ref().is_some_and(|(held, _)| *held);
        let gauges_changed = previous.is_none_or(|resources| resources.gauges != sample.gauges);
        let waits_dirty = full
            || !touched.is_empty()
            || admission
            || area(&QUEUE_AREAS)
            || (held && gauges_changed);
        let (waits, queue_error) = match (&sample.replay, previous) {
            (None, _) => (BTreeMap::new(), None),
            (Some(replay), _) if waits_dirty => parts::waits(replay, &tasks),
            (Some(_), Some(previous)) => (previous.waits.clone(), previous.queue_error.clone()),
            (Some(replay), None) => parts::waits(replay, &tasks),
        };
        let resources = Resources {
            gauges: sample.gauges,
            admission: sample.admission,
            waits,
            queue_error,
        };

        let trackers = plan::refresh(
            read,
            &tasks,
            &self.trackers,
            &plan::Changes {
                full,
                touched: &touched,
                delta: &delta,
                git_projects: &plan.git.projects,
                admission,
            },
        )?;
        let stages = (full || !changed.is_empty() || plan.stages_due).then(|| {
            let stages = parts::per_task(
                &tasks,
                &self.stages,
                "classic.stages",
                |task| {
                    full || changed.contains(&task.id) || (plan.stages_due && parts::gating(task))
                },
                |task| parts::stage(read, task),
            );
            let gating = tasks.iter().any(|task| parts::gating(task));
            (stages, gating, full || plan.stages_due)
        });
        let automation =
            (full || !changed.is_empty() || !plan.git.tasks.is_empty() || policy || admission)
                .then(|| {
                    parts::per_task(
                        &tasks,
                        &self.automation,
                        "classic.automation",
                        |task| {
                            full || policy
                                || admission
                                || changed.contains(&task.id)
                                || plan.git.tasks.contains(&task.id)
                        },
                        |task| parts::automation(read, task),
                    )
                });
        let plan_review = (full || !changed.is_empty() || policy || admission).then(|| {
            parts::per_task(
                &tasks,
                &self.plan_review,
                "classic.plan_review",
                |task| full || policy || admission || changed.contains(&task.id),
                |task| parts::plan_review(read, task),
            )
        });
        let attention =
            (!plan.supplied && (full || delta.any() || plan.attention_due)).then(|| {
                let _span = corral_core::timing::span("classic.attention");
                attention_at(read, plan.now)
            });
        Ok(Computed {
            seq: delta.through,
            full,
            listing,
            trackers,
            resources,
            stages,
            automation,
            plan_review,
            attention,
        })
    }

    fn apply(
        &mut self,
        computed: Computed,
        attention: Option<Attention>,
        now: Timestamp,
    ) -> Result<Option<Arc<ClassicFrame>>> {
        let Computed {
            seq,
            full,
            listing,
            trackers,
            resources,
            stages,
            automation,
            plan_review,
            attention: _,
        } = computed;
        self.checked_through = Some(seq);
        if full {
            self.last_full = Some(now);
        }
        let mut changed = false;
        if let Some(listing) = listing {
            let rows: Vec<u64> = listing.rows.iter().map(|row| row.digest).collect();
            changed |= self.settle(Part::Rows, digest(&rows)?);
            changed |= self.settle(Part::Meta, digest(&listing.meta)?);
            self.rows = listing
                .rows
                .iter()
                .map(|row| (row.id.clone(), Arc::clone(row)))
                .collect();
            self.order = listing.rows;
            self.meta = Some(listing.meta);
        }
        let plan: Vec<(&String, u64)> = trackers
            .iter()
            .map(|(id, tracker)| (id, tracker.digest))
            .collect();
        changed |= self.settle(Part::Plan, digest(&plan)?);
        self.trackers = trackers;
        changed |= self.settle(
            Part::Resources,
            digest(&(&resources.gauges, &resources.queue_error, &resources.waits))?,
        );
        self.resources = Some(resources);
        self.gauges_at = Some(now);
        if let Some((stages, gating, timed)) = stages {
            changed |= self.settle(Part::Stages, digest(&stages)?);
            self.stages = stages;
            self.gating = gating;
            if timed {
                self.stages_at = Some(now);
            }
        }
        if let Some(automation) = automation {
            changed |= self.settle(Part::Automation, digest(&automation)?);
            self.automation = automation;
        }
        if let Some(plan_review) = plan_review {
            changed |= self.settle(Part::PlanReview, digest(&plan_review)?);
            self.plan_review = plan_review;
        }
        if let Some(attention) = attention {
            changed |= self.settle(
                Part::Attention,
                digest(&(&attention.attention, &attention.operating))?,
            );
            self.attention = Some(attention);
            self.attention_at = Some(now);
        }
        if !changed && self.frame.is_some() {
            return Ok(None);
        }
        self.rev += 1;
        let frame = Arc::new(self.build_frame(seq, now)?);
        self.frame = Some(Arc::clone(&frame));
        Ok(Some(frame))
    }

    /// Whether `part`'s content differs from the last one recorded.
    fn settle(&mut self, part: Part, digest: u64) -> bool {
        self.digests.insert(part, digest) != Some(digest)
    }

    fn build_frame(&self, seq: ChangeSeq, now: Timestamp) -> Result<ClassicFrame> {
        let attention = self
            .attention
            .as_ref()
            .context("classic frame has no attention part")?;
        let meta = self.meta.as_ref().context("classic frame has no listing")?;
        let resources = self
            .resources
            .as_ref()
            .context("classic frame has no resource sample")?;
        let mut gauges = resources.gauges.clone();
        gauges.gauges.extend(resources.queue_error.clone());
        Ok(ClassicFrame {
            rev: self.rev,
            seq: seq.0,
            as_of: now,
            attention: attention.attention.clone(),
            operating: attention.operating.clone(),
            heads: self.order.iter().map(|row| row.wire.clone()).collect(),
            diffs: self
                .order
                .iter()
                .filter_map(|row| Some((row.id.clone(), row.diff.clone()?)))
                .collect(),
            obligations: self
                .order
                .iter()
                .filter_map(|row| Some((row.id.clone(), row.obligations.clone()?)))
                .collect(),
            status_counts: meta.status_counts.clone(),
            slot_usage: meta.slot_usage.clone(),
            done_ids: meta.done_ids.clone(),
            pins: meta.pins.clone(),
            finishes: self
                .order
                .iter()
                .filter_map(|row| Some((row.id.clone(), row.finish.clone()?)))
                .collect(),
            unreviewed: self
                .order
                .iter()
                .filter_map(|row| Some((row.id.clone(), row.unreviewed?)))
                .collect(),
            plan_progress: self
                .trackers
                .iter()
                .map(|(id, tracker)| (id.clone(), tracker.progress.clone()))
                .collect(),
            resources: gauges,
            waits: resources.waits.clone(),
            stages: self.stages.clone(),
            automation_blocks: self.automation.clone(),
            plan_review_waits: self.plan_review.clone(),
        })
    }

    /// Worktrees of Review and Running tasks' current runs, and the repositories of
    /// trackers' projects with `main`, the project base and Plan integration branches.
    fn git_targets(&self, ctx: &Ctx) -> Vec<GitTarget> {
        let mut targets = Vec::new();
        let mut repos: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for row in &self.order {
            let task = &row.shell;
            if matches!(task.status, Status::Review | Status::Running)
                && let Some(run) = task.current_run()
            {
                targets.push(GitTarget::Worktree {
                    task: task.id.clone(),
                    path: run.worktree.clone(),
                });
            }
            if task.tracker
                && let Some(scope) = &task.approved_scope
                && let Some(project) = task.project.as_deref()
            {
                let branches = repos.entry(project).or_default();
                branches.insert("main".into());
                if let PlanDeliveryTarget::Integration { branch } = &scope.delivery_target {
                    branches.insert(branch.clone());
                }
            }
        }
        for (project, mut branches) in repos {
            let Ok(path) = ctx.store.project_path(project) else {
                continue;
            };
            if let Some(base) = ctx
                .store
                .config
                .projects
                .get(project)
                .and_then(|project| project.base.clone())
            {
                branches.insert(base);
            }
            targets.push(GitTarget::Repo {
                project: project.to_owned(),
                path,
                branches: branches.into_iter().collect(),
            });
        }
        targets
    }
}

/// Shell tasks in `heads` order: `to_shell_task` plus the `diffs` artifact, as `FrameSources` did.
pub fn shells(frame: &ClassicFrame) -> Result<Vec<corral_core::Task>> {
    frame
        .heads
        .iter()
        .map(|wire| {
            let head = TaskHead::from_wire(wire.clone());
            let mut shell = head.to_shell_task()?;
            if head.current_run.is_some() {
                shell.artifacts.extend(frame.diffs.get(&head.id).cloned());
            }
            Ok(shell)
        })
        .collect()
}

/// Content identity of one part; equal content gives equal digests within a process.
fn digest<T: Serialize + ?Sized>(value: &T) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    let bytes = serde_json::to_vec(value).context("encoding a classic frame part")?;
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(hasher.finish())
}

/// Replays one host observation, including an unavailable one.
#[derive(Debug)]
struct SampleProbe(Result<ResourceSnapshot, String>);

impl crate::resources::ResourceProbe for SampleProbe {
    fn sample(&self, _: &corral_core::Store) -> Result<ResourceSnapshot> {
        self.0.clone().map_err(anyhow::Error::msg)
    }
}

/// Gauges and attention share one observation; the next refresh samples again.
fn sampled(ctx: &Ctx) -> Ctx {
    let observation = ctx
        .resource_probe
        .sample(&ctx.store)
        .map_err(|error| format!("{error:#}"));
    Ctx {
        resource_probe: Arc::new(SampleProbe(observation)),
        ..ctx.clone()
    }
}

/// The attention board at `now`. A partial listing must never become a falsely reassuring
/// attention count: a file diagnostic makes the whole read unavailable.
fn attention_at(ctx: &Ctx, now: Timestamp) -> Result<AttentionBoard, String> {
    let (sender, messages) = std::sync::mpsc::channel();
    let result = {
        let _capture = corral_core::capture_messages(sender);
        crate::attention::board_hot(ctx, now)
    };
    let mut unavailable = None;
    for message in messages.try_iter() {
        if message.file.is_some() {
            unavailable.get_or_insert(message.text);
        } else {
            corral_core::report_message(message.task.as_deref(), message.source, message.text);
        }
    }
    if let Some(error) = unavailable {
        return Err(error);
    }
    result.map_err(|error| format!("{error:#}"))
}

/// The operating header read back with the frame's attention, as `board_readback` does.
fn operating(ctx: &Ctx, attention: &Result<AttentionBoard, String>) -> String {
    let workset = attention
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
        .and_then(|board| {
            let _span = corral_core::timing::span("board.workset");
            crate::pm_workset_with_attention(ctx, board)
        });
    crate::pm_read::readback_result(workset)
}

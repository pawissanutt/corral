//! The store-derived parts of a classic frame, ported from the classic board's snapshot
//! (`FrameSources::load`, `active_stages`, `ResourceDisplay::sample_board`). Each function
//! keeps the board's field semantics exactly; the cache in `mod.rs` decides which run.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::Result;
use corral_core::backend::{HeadQuery, StatusCount};
use corral_core::{
    Artifact, ArtifactKind, Config, HeadWire, PinRef, PinTarget, ResourceState, RunObligations,
    Status, Task, TaskHead,
};

use super::{ClassicPin, Gauge, GaugeLevel, ResourceGauges, digest};
use crate::resources::FixedResourceProbe;
use crate::{Ctx, Finish, SlotUsage};

/// One head of the frame and everything derived from it alone.
#[derive(Debug)]
pub(super) struct Row {
    pub(super) id: String,
    pub(super) rev: u64,
    pub(super) wire: HeadWire,
    /// `to_shell_task` plus the latest Diff artifact of the current run.
    pub(super) shell: Task,
    pub(super) diff: Option<Artifact>,
    pub(super) obligations: Option<RunObligations>,
    pub(super) unreviewed: Option<usize>,
    pub(super) finish: Option<Finish>,
    pub(super) digest: u64,
}

/// `status_counts`, `slot_usage`, `done_ids` and `pins`.
#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct Meta {
    pub(super) status_counts: Vec<StatusCount>,
    pub(super) slot_usage: SlotUsage,
    pub(super) done_ids: BTreeSet<String>,
    pub(super) pins: Vec<ClassicPin>,
}

/// One listing of the frame's heads: rows in task-number order, the ids whose row was
/// recomputed (or departed), and the meta part read from the same board.
pub(super) struct Listing {
    pub(super) rows: Vec<Arc<Row>>,
    pub(super) changed: BTreeSet<String>,
    pub(super) meta: Meta,
}

/// Which cached rows a listing may reuse.
pub(super) struct Reuse<'a> {
    pub(super) previous: &'a BTreeMap<String, Arc<Row>>,
    /// Tasks named by any feed scope since the cached rows were computed.
    pub(super) touched: &'a BTreeSet<String>,
    /// An aggregate changed: reviewed-artifact counts of Review heads may differ.
    pub(super) aggregates: bool,
    pub(super) full: bool,
}

/// The board's open heads, the configured Done page and pinned heads, as
/// `FrameSources::load` read them; rows whose head revision and feed scopes are unchanged
/// are reused.
pub(super) fn listing(read: &Ctx, reuse: &Reuse<'_>) -> Result<Listing> {
    let _span = corral_core::timing::span("classic.rows");
    let board = read.store.cached_board()?;
    let meta_span = corral_core::timing::span("classic.meta");
    let status_counts = board.status_counts;
    let slot_usage = SlotUsage::from_heads(&board.heads, &read.store.config.limits);
    drop(meta_span);
    let mut heads = board.heads;
    // The configured Done viewport bounds the page independently of terminal history.
    heads.extend(read.store.heads_page(
        &HeadQuery::RecentByStatus {
            status: "done".into(),
        },
        u32::try_from(read.store.config.ui.done_max).unwrap_or(u32::MAX),
    )?);
    let meta_span = corral_core::timing::span("classic.meta");
    let pins = load_pins(read);
    let pin_ids = pins
        .iter()
        .filter_map(|entry| PinRef::parse(&entry.pin.target).ok())
        .map(|pin| pin.task().to_owned())
        .filter(|id| !heads.iter().any(|h| h.id == *id))
        .collect();
    heads.extend(read.store.heads(&HeadQuery::Ids(pin_ids))?);
    let relation_ids = heads
        .iter()
        .flat_map(|h| h.depends_on.iter().chain(&h.children))
        .cloned()
        .collect();
    let done_ids = read
        .store
        .heads(&HeadQuery::Ids(relation_ids))?
        .into_iter()
        .filter(|h| h.status == Status::Done)
        .map(|h| h.id.clone())
        .collect();
    drop(meta_span);
    heads.sort_by_key(|head| head.num);
    let (rows, changed) = rows(read, heads, reuse)?;
    Ok(Listing {
        rows,
        changed,
        meta: Meta {
            status_counts,
            slot_usage,
            done_ids,
            pins,
        },
    })
}

fn reusable<'a>(head: &TaskHead, reuse: &Reuse<'a>) -> Option<&'a Arc<Row>> {
    if reuse.full
        || reuse.touched.contains(&head.id)
        || (reuse.aggregates && head.status == Status::Review)
    {
        return None;
    }
    reuse
        .previous
        .get(&head.id)
        .filter(|row| row.rev == head.rev)
}

fn rows(
    read: &Ctx,
    heads: Vec<TaskHead>,
    reuse: &Reuse<'_>,
) -> Result<(Vec<Arc<Row>>, BTreeSet<String>)> {
    let mut slots: Vec<Option<Arc<Row>>> = Vec::with_capacity(heads.len());
    let mut fresh = Vec::new();
    for (index, head) in heads.iter().enumerate() {
        match reusable(head, reuse) {
            Some(row) => slots.push(Some(Arc::clone(row))),
            None => {
                slots.push(None);
                fresh.push(index);
            }
        }
    }
    let terminal: Vec<String> = fresh
        .iter()
        .map(|&index| &heads[index])
        .filter(|h| matches!(h.status, Status::Done | Status::Dropped))
        .map(|h| h.id.clone())
        .collect();
    let mut finishes = if terminal.is_empty() {
        std::collections::HashMap::new()
    } else {
        crate::finishes_for(&read.store, &terminal)?
    };
    let mut changed = BTreeSet::new();
    for index in fresh {
        let head = &heads[index];
        let mut shell = head.to_shell_task()?;
        let (diff, obligations) = match &head.current_run {
            Some(run) => {
                let diff = head.latest_artifact_kind(&run.id, ArtifactKind::Diff)?;
                shell.artifacts.extend(diff.clone());
                (diff, Some(head.card_obligations(&run.id)?))
            }
            None => (None, None),
        };
        let unreviewed = if head.status == Status::Review {
            let count = head
                .summary
                .kept_artifacts
                .saturating_sub(head.reviewed_keep_artifacts()?);
            (count > 0).then(|| usize::try_from(count).unwrap_or(usize::MAX))
        } else {
            None
        };
        let finish = finishes.remove(&head.id);
        let wire = head.to_wire();
        let digest = digest(&(&wire, &diff, &obligations, &unreviewed, &finish))?;
        changed.insert(head.id.clone());
        slots[index] = Some(Arc::new(Row {
            id: head.id.clone(),
            rev: head.rev,
            wire,
            shell,
            diff,
            obligations,
            unreviewed,
            finish,
            digest,
        }));
    }
    let rows: Vec<Arc<Row>> = slots.into_iter().flatten().collect();
    let present: BTreeSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    changed.extend(
        reuse
            .previous
            .keys()
            .filter(|id| !present.contains(id.as_str()))
            .cloned(),
    );
    Ok((rows, changed))
}

/// Resolve only the heads and keyed artifacts actually named by shelf pins.
fn load_pins(read: &Ctx) -> Vec<ClassicPin> {
    read.store
        .pins()
        .unwrap_or_default()
        .into_iter()
        .map(|pin| {
            let target = read
                .store
                .resolve_pin_head(&pin)
                .unwrap_or_else(|error| PinTarget::Broken(format!("{error:#}")));
            ClassicPin { pin, target }
        })
        .collect()
}

/// The admission inputs a resource sample decides: `(held, reason)`, or None when the
/// host or the resource status could not be read.
pub(super) type Admission = Option<(bool, Option<String>)>;

/// One host observation turned into gauges; `replay` replays it for the dispatch queue.
pub(super) struct Sampled {
    pub(super) gauges: ResourceGauges,
    pub(super) admission: Admission,
    pub(super) replay: Option<Ctx>,
}

/// The gauge half of `ResourceDisplay::sample_board`: the sample is replayed so the gauges
/// and every Ready card's waiting reason agree.
pub(super) fn sample(read: &Ctx) -> Sampled {
    let _span = corral_core::timing::span("classic.resources");
    let config = &read.store.config;
    let snapshot = match read.resource_probe.sample(&read.store) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Sampled {
                gauges: gauges(Err(error), config),
                admission: None,
                replay: None,
            };
        }
    };
    let replay = Ctx {
        resource_probe: Arc::new(FixedResourceProbe(snapshot)),
        ..read.clone()
    };
    let status = crate::resources::status(&replay);
    let admission = status
        .as_ref()
        .ok()
        .map(|state| (state.held, state.reason.clone()));
    Sampled {
        gauges: gauges(status, config),
        admission,
        replay: Some(replay),
    }
}

/// The dispatch-queue half of `sample_board`: each Ready task's waiting reason, or the
/// gauge that says the queue could not be read.
pub(super) fn waits(replay: &Ctx, tasks: &[&Task]) -> (BTreeMap<String, String>, Option<Gauge>) {
    let _span = corral_core::timing::span("classic.resources");
    match crate::dispatch_queue(replay) {
        Ok(entries) => (
            entries
                .into_iter()
                .filter(|e| {
                    tasks
                        .iter()
                        .any(|t| t.id == e.id && t.status == Status::Ready)
                })
                .map(|e| (e.id, e.reason))
                .collect(),
            None,
        ),
        Err(error) => (
            BTreeMap::new(),
            Some(Gauge {
                text: format!("queue unavailable: {error:#}"),
                level: GaugeLevel::Bad,
            }),
        ),
    }
}

/// `ResourceDisplay::from_sample` with levels instead of colours.
fn gauges(sample: Result<ResourceState>, config: &Config) -> ResourceGauges {
    let (state, min_mem, min_disk) = match sample.and_then(|state| {
        Ok((
            state,
            config.resources.min_mem_bytes()?,
            config.resources.min_disk_bytes()?,
        ))
    }) {
        Ok(values) => values,
        Err(error) => {
            return ResourceGauges {
                reason: None,
                gauges: vec![gauge(
                    format!("resources unavailable: {error:#}"),
                    GaugeLevel::Warn,
                )],
            };
        }
    };
    let s = &state.snapshot;
    let load = if s.load_per_core > config.resources.max_load_per_core {
        GaugeLevel::Bad
    } else if s.load_per_core >= config.resources.max_load_per_core * 0.8 {
        GaugeLevel::Warn
    } else {
        GaugeLevel::Good
    };
    let low = |available: u64, minimum: u64| {
        if available < minimum {
            GaugeLevel::Bad
        } else if available < minimum.saturating_add(minimum / 4) {
            GaugeLevel::Warn
        } else {
            GaugeLevel::Good
        }
    };
    let disk = s.disks.iter().map(|d| d.free_bytes).min();
    let mut gauges = vec![
        gauge(format!("load {:.2}/core", s.load_per_core), load),
        gauge(
            format!("mem {}", bytes(s.mem_available)),
            low(s.mem_available, min_mem),
        ),
        gauge(
            disk.map_or_else(|| "disk n/a".into(), |free| format!("disk {}", bytes(free))),
            disk.map_or(GaugeLevel::Warn, |free| low(free, min_disk)),
        ),
    ];
    if state.held {
        gauges.push(if state.severe {
            gauge("held: needs you".into(), GaugeLevel::Bad)
        } else {
            gauge("held".into(), GaugeLevel::Warn)
        });
    }
    ResourceGauges {
        reason: state.reason,
        gauges,
    }
}

fn gauge(text: String, level: GaugeLevel) -> Gauge {
    Gauge { text, level }
}

fn bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{}GiB", bytes / 1_073_741_824)
    } else {
        format!("{}MiB", bytes / 1_048_576)
    }
}

/// Whether a task's current run holds a gate slot: its stage text follows the gate's
/// display-only progress files, which change without a store write.
pub(super) fn gating(task: &Task) -> bool {
    task.current_run()
        .and_then(|run| run.gate.as_ref())
        .is_some_and(|gate| gate.holds_slot())
}

/// One entry of `active_stages`: a reviewing model, or the running gate command.
pub(super) fn stage(ctx: &Ctx, task: &Task) -> Option<String> {
    use crate::watch::ProgressState;
    let run = task.current_run()?;
    if let Some(review) = &run.review
        && review.holds_slot()
    {
        let model = run
            .review_runs
            .last()
            .and_then(|r| r.model.as_deref())
            .unwrap_or(&review.reviewer);
        return Some(format!("reviewing: {model}"));
    }
    if run.gate.as_ref().is_some_and(|g| g.holds_slot()) {
        let history = match crate::watch::load(ctx, &task.id) {
            Ok(history) => history,
            Err(error) => {
                crate::report_message(Some(&task.id), "stage progress", format!("{error:#}"));
                return Some("gating: progress unavailable".into());
            }
        };
        if let Some(gate) = history.gates.last() {
            let active = gate
                .commands
                .iter()
                .enumerate()
                .find(|(_, s)| s.state == ProgressState::Running);
            return Some(if let Some((i, command)) = active {
                format!(
                    "gating {}/{}: {}",
                    i + 1,
                    gate.commands.len(),
                    command.command
                )
            } else {
                format!("gating: {}", gate.state.label())
            });
        }
    }
    None
}

/// Why automatic review processing cannot take its next step, as the board shows it.
pub(super) fn automation(ctx: &Ctx, task: &Task) -> Option<String> {
    crate::automation_blocker(ctx, task)
        .unwrap_or_else(|error| Some(format!("automation unavailable: {error:#}")))
}

/// Why automation will not start the review of a submitted plan by itself.
pub(super) fn plan_review(ctx: &Ctx, task: &Task) -> Option<String> {
    crate::plan_review_wait(ctx, task)
}

/// Recompute `compute(task)` for the tasks `dirty` selects and reuse `previous` for the rest;
/// tasks that left the frame are dropped. Each recomputed task enters `span` once.
pub(super) fn per_task(
    tasks: &[&Task],
    previous: &BTreeMap<String, String>,
    span: &'static str,
    mut dirty: impl FnMut(&Task) -> bool,
    mut compute: impl FnMut(&Task) -> Option<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for &task in tasks {
        let value = if dirty(task) {
            let _span = corral_core::timing::span(span);
            compute(task)
        } else {
            previous.get(&task.id).cloned()
        };
        if let Some(value) = value {
            out.insert(task.id.clone(), value);
        }
    }
    out
}

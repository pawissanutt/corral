//! Board data from a classic frame, computed by the daemon or by a local `ClassicCache` on the
//! sync worker, and applied without store I/O.
use anyhow::Context as _;
use corral_ops::classic::{ClassicCache, ClassicFrame, ClassicInputs};

use super::*;

/// An owned board refresh. UI-only state (selection, scroll, overlays and clock) stays in the view.
pub struct BoardSnapshot {
    pub(crate) attention: crate::attention::Snapshot,
    pub(crate) ctx: Ctx,
    pub(crate) tasks: Vec<Task>,
    heads: HashMap<String, corral_core::TaskHead>,
    obligations: HashMap<String, corral_core::RunObligations>,
    pub(crate) status_counts: Vec<corral_core::backend::StatusCount>,
    pub(crate) slot_usage: SlotUsage,
    done_ids: HashSet<String>,
    pub(crate) resources: crate::resources::ResourceDisplay,
    pub(crate) operating_header: String,
    finishes: HashMap<String, Finish>,
    waits: HashMap<String, String>,
    pins: Vec<shelf::Entry>,
    pinned: HashSet<String>,
    stages: HashMap<String, String>,
    automation_blocks: HashMap<String, String>,
    plan_review_waits: HashMap<String, String>,
    unreviewed: HashMap<String, usize>,
    plan_progress: HashMap<String, crate::plan::tracker::Progress>,
}

impl BoardSnapshot {
    #[cfg(test)]
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        Self::load_at(ctx, Timestamp::now())
    }

    /// A full local frame at `now`, computed from scratch.
    pub(super) fn load_at(ctx: &Ctx, now: Timestamp) -> Result<Self> {
        Self::load_cached(
            ctx,
            &mut ClassicCache::new(),
            ClassicInputs {
                now,
                attention: None,
                force: true,
            },
        )
    }

    /// Refreshes the caller's local frame cache, then applies its current frame.
    pub(crate) fn load_cached(
        ctx: &Ctx,
        cache: &mut ClassicCache,
        inputs: ClassicInputs,
    ) -> Result<Self> {
        cache.refresh(ctx, inputs)?;
        let frame = cache
            .current()
            .context("classic frame cache has no frame after a refresh")?;
        Self::from_frame(ctx, ClassicFrame::clone(&frame))
    }

    /// The only constructor besides tests; daemon and local paths both end here. Reads no store.
    pub(crate) fn from_frame(ctx: &Ctx, frame: ClassicFrame) -> Result<Self> {
        let _span = corral_core::timing::span("board.frame.apply");
        let tasks = corral_ops::classic::shells(&frame)?;
        let ClassicFrame {
            attention,
            operating,
            heads,
            obligations,
            status_counts,
            slot_usage,
            done_ids,
            pins,
            finishes,
            unreviewed,
            plan_progress,
            resources,
            waits,
            stages,
            automation_blocks,
            plan_review_waits,
            ..
        } = frame;
        let pins: Vec<shelf::Entry> = pins
            .into_iter()
            .map(|pin| shelf::Entry {
                pin: pin.pin,
                target: pin.target,
            })
            .collect();
        let pinned = pins
            .iter()
            .filter_map(|entry| PinRef::parse(&entry.pin.target).ok())
            .map(|reference| reference.task().to_string())
            .collect();
        Ok(Self {
            attention: attention.and_then(crate::attention::from_board),
            ctx: ctx.clone(),
            tasks,
            heads: heads
                .into_iter()
                .map(|wire| {
                    let head = corral_core::TaskHead::from_wire(wire);
                    (head.id.clone(), head)
                })
                .collect(),
            obligations: obligations.into_iter().collect(),
            status_counts,
            slot_usage,
            done_ids: done_ids.into_iter().collect(),
            resources: crate::resources::ResourceDisplay::from_gauges(&resources),
            operating_header: operating,
            finishes: finishes.into_iter().collect(),
            waits: waits.into_iter().collect(),
            pins,
            pinned,
            stages: stages.into_iter().collect(),
            automation_blocks: automation_blocks.into_iter().collect(),
            plan_review_waits: plan_review_waits.into_iter().collect(),
            unreviewed: unreviewed.into_iter().collect(),
            plan_progress: plan_progress.into_iter().collect(),
        })
    }

    pub(crate) fn stage_reason(&self, change: &corral_ops::Change) -> Option<String> {
        let stage = change.stage?;
        let task = self.tasks.iter().find(|task| task.id == change.task)?;
        let state = task.current_run()?.autopilot.as_ref()?;
        (state.stage == stage)
            .then(|| state.reason.clone())
            .flatten()
    }
}

impl BoardView {
    /// Only transfers owned data and restores UI state. Route discovery remains on its own worker;
    /// `refresh_routes` queues task/config revisions and never reads the store.
    pub(crate) fn apply_snapshot(&mut self, snapshot: BoardSnapshot) {
        self.attention = snapshot.attention;
        self.tasks = snapshot.tasks;
        self.heads = snapshot.heads;
        self.obligations = snapshot.obligations;
        self.status_counts = snapshot.status_counts;
        self.slot_usage = snapshot.slot_usage;
        self.finishes = snapshot.finishes;
        self.resources = snapshot.resources;
        self.waits = snapshot.waits;
        self.pins = snapshot.pins;
        self.pinned = snapshot.pinned;
        self.stages = snapshot.stages;
        self.automation_blocks = snapshot.automation_blocks;
        self.plan_review_waits = snapshot.plan_review_waits;
        self.unreviewed = snapshot.unreviewed;
        self.plan_progress = snapshot.plan_progress;
        self.index =
            index::BoardIndex::build(&self.tasks, &self.finishes, &self.pinned, &self.attention);
        self.index.done_ids.extend(snapshot.done_ids);
        if let Some(cursor) = &mut self.shelf_cursor {
            *cursor = (*cursor).min(self.pins.len().saturating_sub(1));
        }
        if self.pins.is_empty() {
            self.shelf_cursor = None;
        }
        self.refresh_routes(&snapshot.ctx);
        let ui = &snapshot.ctx.store.config.ui;
        self.done_window = ui.done_window();
        self.done_max = ui.done_max;
        if self.visible_columns != ui.columns {
            self.visible_columns = ui.columns.clone();
            self.show_done = self.visible_columns.contains(&Status::Done);
        }
        self.restore_selection();
        release_refresh_pages();
    }
}

/// A refresh temporarily holds two frames plus JSON conversion buffers. Once the old frame
/// is retired, return unused allocator pages instead of retaining that high-water RSS for
/// the board's lifetime. This is not a cache eviction and never runs on paint/input paths.
fn release_refresh_pages() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: glibc's thread-safe advisory trim has no pointer/lifetime preconditions.
        // Zero retains no extra top padding; live allocations and other threads are untouched.
        unsafe { malloc_trim(0) };
    }
}

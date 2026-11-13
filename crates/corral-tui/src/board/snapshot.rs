//! Store-backed board data, loaded on the sync worker and applied without store I/O.
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

    pub(super) fn load_at(ctx: &Ctx, now: Timestamp) -> Result<Self> {
        Self::load_refresh(ctx, now, None)
    }

    pub(crate) fn load_with_attention(
        ctx: &Ctx,
        attention: crate::attention::Snapshot,
    ) -> Result<Self> {
        Self::load_refresh(ctx, Timestamp::now(), Some(attention))
    }

    fn load_refresh(
        ctx: &Ctx,
        now: Timestamp,
        attention: Option<crate::attention::Snapshot>,
    ) -> Result<Self> {
        // Preparation may wait on derivation locks or do substantial filesystem work.
        // Never age a frame view while it runs. Sample the host once, outside the
        // retryable read derivation; all later admission reads replay that observation.
        let _span = corral_core::timing::span("board.snap.prepare");
        corral_ops::lesson_rows::prepare_coordination(ctx)?;
        let _hot = corral_core::work::HotScope::enter();
        let sampled = crate::attention::refresh_context(ctx);
        drop(_span);
        sampled.read_step(|read| Self::load_view(read, ctx.clone(), now, attention.clone()))
    }

    fn load_view(
        ctx: &Ctx,
        live_ctx: Ctx,
        now: Timestamp,
        attention: Option<crate::attention::Snapshot>,
    ) -> Result<Self> {
        let (sources, finishes, unreviewed) = ctx.read_step(|read| {
            let _span = corral_core::timing::span("board.snap.sources");
            let mut sources = sources::FrameSources::load(read)?;
            drop(_span);
            let _span = corral_core::timing::span("board.snap.finishes");
            let finishes = corral_ops::finishes_for(
                &read.store,
                &sources
                    .heads
                    .iter()
                    .filter(|h| matches!(h.status, Status::Done | Status::Dropped))
                    .map(|h| h.id.clone())
                    .collect::<Vec<_>>(),
            )?;
            drop(_span);
            let _span = corral_core::timing::span("board.snap.unreviewed");
            let mut unreviewed = HashMap::new();
            for head in &sources.heads {
                if head.status == Status::Review {
                    let count = head
                        .summary
                        .kept_artifacts
                        .saturating_sub(head.reviewed_keep_artifacts()?);
                    if count > 0 {
                        unreviewed.insert(
                            head.id.clone(),
                            usize::try_from(count).unwrap_or(usize::MAX),
                        );
                    }
                }
            }
            drop(_span);
            sources.heads = sources
                .heads
                .into_iter()
                .map(corral_core::TaskHead::detach)
                .collect();
            Ok((sources, finishes, unreviewed))
        })?;
        let sources::FrameSources {
            tasks,
            heads,
            obligations,
            status_counts,
            slot_usage,
            done_ids,
            pins,
        } = sources;
        let _span = corral_core::timing::span("board.snap.plan_progress");
        let plan_progress = tasks
            .iter()
            .filter_map(|task| crate::plan::tracker::load(ctx, task).map(|p| (task.id.clone(), p)))
            .collect();
        drop(_span);
        let _span = corral_core::timing::span("board.snap.resources");
        let resource_ctx = crate::attention::refresh_context(ctx);
        let (resources, waits) =
            crate::resources::ResourceDisplay::sample_board(&resource_ctx, &tasks);
        drop(_span);
        let pinned = pins
            .iter()
            .filter_map(|entry| PinRef::parse(&entry.pin.target).ok())
            .map(|reference| reference.task().to_string())
            .collect();
        let _span = corral_core::timing::span("board.snap.stages");
        let stages = autopilot::active_stages(ctx, &tasks);
        drop(_span);
        let _span = corral_core::timing::span("board.snap.automation");
        let automation_blocks = tasks
            .iter()
            .filter_map(|task| {
                let reason = corral_ops::automation_blocker(ctx, task)
                    .unwrap_or_else(|error| Some(format!("automation unavailable: {error:#}")));
                reason.map(|reason| (task.id.clone(), reason))
            })
            .collect();
        drop(_span);
        let _span = corral_core::timing::span("board.snap.plan_review");
        let plan_review_waits = tasks
            .iter()
            .filter_map(|task| Some((task.id.clone(), corral_ops::plan_review_wait(ctx, task)?)))
            .collect();
        drop(_span);
        Ok(Self {
            attention: attention.unwrap_or_else(|| {
                let _span = corral_core::timing::span("board.snap.attention");
                crate::attention::load(&resource_ctx, now)
            }),
            ctx: live_ctx,
            tasks,
            heads: heads
                .into_iter()
                .map(|h| (h.id.clone(), h.detach()))
                .collect(),
            obligations,
            status_counts,
            slot_usage,
            done_ids,
            resources,
            operating_header: String::new(),
            finishes,
            waits,
            pins,
            pinned,
            stages,
            automation_blocks,
            plan_review_waits,
            unreviewed,
            plan_progress,
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

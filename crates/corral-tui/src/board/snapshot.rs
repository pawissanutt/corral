//! Store-backed board data, loaded on the sync worker and applied without store I/O.
use super::*;

/// An owned board refresh. UI-only state (selection, scroll, overlays and clock) stays in the view.
pub struct BoardSnapshot {
    attention: crate::attention::Snapshot,
    pub(crate) ctx: Ctx,
    pub(crate) tasks: Vec<Task>,
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
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        Self::load_at(ctx, Timestamp::now())
    }

    pub(super) fn load_at(ctx: &Ctx, now: Timestamp) -> Result<Self> {
        let tasks = ctx.store.list()?;
        let plan_progress = tasks
            .iter()
            .filter_map(|task| crate::plan::tracker::load(ctx, task).map(|p| (task.id.clone(), p)))
            .collect();
        let finishes = corral_ops::finishes(&ctx.store, &tasks)?;
        let (resources, waits) = crate::resources::ResourceDisplay::sample_board(ctx, &tasks);
        let pins = shelf::load(ctx, &tasks);
        let pinned = pins
            .iter()
            .filter_map(|entry| PinRef::parse(&entry.pin.target).ok())
            .map(|reference| reference.task().to_string())
            .collect();
        let stages = autopilot::active_stages(ctx, &tasks);
        let automation_blocks = tasks
            .iter()
            .filter_map(|task| {
                let reason = corral_ops::automation_blocker(ctx, task)
                    .unwrap_or_else(|error| Some(format!("automation unavailable: {error:#}")));
                reason.map(|reason| (task.id.clone(), reason))
            })
            .collect();
        let plan_review_waits = tasks
            .iter()
            .filter_map(|task| Some((task.id.clone(), corral_ops::plan_review_wait(ctx, task)?)))
            .collect();
        let reviewed = corral_ops::reviewed_set(ctx);
        let unreviewed = tasks
            .iter()
            .filter(|task| task.status == Status::Review)
            .filter_map(|task| {
                let count = task
                    .artifacts
                    .iter()
                    .filter(|artifact| {
                        artifact.tier() == corral_core::ArtifactTier::Keep
                            && artifact.purged_at.is_none()
                            && !reviewed.contains(&artifact.path)
                    })
                    .count();
                (count > 0).then(|| (task.id.clone(), count))
            })
            .collect();
        Ok(Self {
            attention: crate::attention::load(ctx, now),
            ctx: ctx.clone(),
            tasks,
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
        self.index = index::BoardIndex::build(&self.tasks, &self.finishes, &self.pinned);
        self.index
            .usage(&self.tasks, &snapshot.ctx.store.config.limits);
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
    }
}

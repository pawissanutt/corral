//! Which steps and tasks one sync pass runs. The daemon's incremental planner decides from what
//! changed since its last pass; every other pass runs everything through [`FullGate`].
use corral_core::TaskHead;
use corral_herdr::PaneInfo;

/// One gated step of `sync_scoped`, in call order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum StepId {
    HoldFirst,
    DesktopFirst,
    Digest,
    MigrateBacklog,
    TrackerReconcile,
    ResourcePhase,
    LessonsDue,
    PmPolicy,
    GateCollect,
    ScratchReclaim,
    LifecycleGc,
    OutputsSchedule,
    SweepInventory,
    Obligations,
    HeadLoop,
    NativeLeftover,
    OwnerDelivery,
    PmInbox,
    AgentHooksPass,
    ReviewBudget,
    PostLand,
    HandoffRecovery,
    ResumeGates,
    GateQueue,
    PlanPilot,
    Availability,
    Autopilot,
    AutopilotWorkspace,
    HoldSecond,
    Movement,
    ReacquireWoken,
    StartQueued,
    DispatchPolicy,
    Attention,
    PmEvents,
    DesktopSecond,
    SandboxSweep,
    Usage,
    Supervise,
    MaintainDatabase,
}

/// What a step did, measured by the gate (non-lease feed entries written during it).
#[derive(Debug, Clone, Default)]
pub(crate) struct Effect {
    pub wrote: bool,
    pub changes: usize,
    /// Ids (`T-<n>`, as `TaskHead.id`) of every task a non-lease entry written during the
    /// step names. Empty when the gate does not measure.
    pub tasks: std::collections::BTreeSet<String>,
}

pub(crate) trait StepGate {
    fn run(&mut self, step: StepId) -> bool;
    fn task(&mut self, step: StepId, head: &TaskHead) -> bool;
    fn observe_inventory(&mut self, panes: &[PaneInfo]);
    /// Row 7's admission key `(held, severe, reason)`.
    fn observe_resources(&mut self, _state: &crate::resources::ResourceState) {}
    /// The config the pass runs with (row 1's reload).
    fn observe_config(&mut self, _config: &corral_core::Config) {}
    /// Row 10's `herdr.available()`; a flip makes the rest of the pass full.
    fn observe_herdr(&mut self, _available: bool) {}
    /// Row 15's open heads (after the ownership fence).
    fn observe_heads(&mut self, _heads: &[TaskHead]) {}
    /// Whether the caller must measure effects (costs two view reads per step).
    fn measures(&self) -> bool;
    fn finished(&mut self, step: StepId, effect: Effect);
    fn finished_task(&mut self, step: StepId, task: &str, effect: Effect);
}

/// Every step, every task; measures nothing. Used by all non-daemon passes.
pub(crate) struct FullGate;

impl StepGate for FullGate {
    fn run(&mut self, _step: StepId) -> bool {
        true
    }

    fn task(&mut self, _step: StepId, _head: &TaskHead) -> bool {
        true
    }

    fn observe_inventory(&mut self, _panes: &[PaneInfo]) {}

    fn measures(&self) -> bool {
        false
    }

    fn finished(&mut self, _step: StepId, _effect: Effect) {}

    fn finished_task(&mut self, _step: StepId, _task: &str, _effect: Effect) {}
}

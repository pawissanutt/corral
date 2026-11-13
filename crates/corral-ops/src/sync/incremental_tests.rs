use std::collections::{BTreeMap, BTreeSet};

use corral_core::backend::TaskNum;
use corral_core::{RunHead, Status, Store};
use corral_herdr::{AgentStatus, PaneInfo};
use serde_json::json;

use super::super::task_due::task_due;
use super::rules::STEPS;
use super::*;

fn t0() -> Timestamp {
    "2026-03-02T12:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0() + SignedDuration::from_secs(secs)
}

fn config() -> (tempfile::TempDir, Config) {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config::default_for(tmp.path());
    (tmp, config)
}

fn resources(held: bool) -> ResourceState {
    ResourceState {
        snapshot: crate::resources::FixedResourceProbe::healthy().0,
        reason: held.then(|| "load".to_string()),
        severe: false,
        held,
        observed_at: t0(),
    }
}

fn pane(id: &str) -> PaneInfo {
    PaneInfo {
        pane_id: id.into(),
        workspace_id: "w".into(),
        tab_id: "t".into(),
        agent: None,
        status: AgentStatus::Working,
        title: None,
        cwd: None,
    }
}

fn steps(global: bool) -> impl Iterator<Item = StepId> {
    STEPS
        .into_iter()
        .filter(move |s| rule(*s).per_task != global)
}

/// Same inputs as the warm-up passes: unchanged resources, herdr and inventory.
fn observe(planner: &mut Planner<'_>, heads: &[TaskHead], panes: &[PaneInfo]) {
    planner.observe_resources(&resources(false));
    planner.observe_herdr(true);
    planner.observe_inventory(panes);
    planner.observe_heads(heads);
}

/// Two full passes at `t0` that ran every step and task and acted nowhere.
fn warm(config: &Config, heads: &[TaskHead], panes: &[PaneInfo]) -> PassMemory {
    let mut memory = PassMemory::new();
    for _ in 0..2 {
        let mut planner = memory.plan(
            PassRequest::Full(FullReason::Owner),
            t0(),
            config,
            FeedDelta::default(),
            GitChanges::default(),
            false,
        );
        observe(&mut planner, heads, panes);
        for step in steps(true) {
            assert!(planner.run(step));
        }
        for step in steps(false) {
            for head in heads {
                assert!(planner.task(step, head));
            }
        }
        planner.finish(t0());
    }
    memory
}

fn paced<'m>(
    memory: &'m mut PassMemory,
    config: &Config,
    now: Timestamp,
    feed: FeedDelta,
) -> Planner<'m> {
    memory.plan(
        PassRequest::Paced,
        now,
        config,
        feed,
        GitChanges::default(),
        false,
    )
}

/// The global steps one incremental pass runs.
fn fired(planner: &mut Planner<'_>) -> BTreeSet<StepId> {
    assert_eq!(planner.full, None);
    steps(true).filter(|s| planner.run(*s)).collect()
}

fn wrote() -> Effect {
    Effect {
        wrote: true,
        ..Effect::default()
    }
}

fn feed_areas(area: &str, key: &str) -> FeedDelta {
    FeedDelta {
        areas: BTreeMap::from([(area.to_string(), BTreeSet::from([key.to_string()]))]),
        ..FeedDelta::default()
    }
}

use super::super::gate::StepId::*;

const ANY_ROWS: [StepId; 12] = [
    HoldFirst,
    OwnerDelivery,
    PmInbox,
    AgentHooksPass,
    ResumeGates,
    GateQueue,
    Availability,
    AutopilotWorkspace,
    HoldSecond,
    Attention,
    Supervise,
    MaintainDatabase,
];

const TASK_ROWS: [StepId; 18] = [
    DesktopFirst,
    MigrateBacklog,
    TrackerReconcile,
    ResourcePhase,
    GateCollect,
    ScratchReclaim,
    LifecycleGc,
    SweepInventory,
    ReviewBudget,
    PostLand,
    HandoffRecovery,
    Movement,
    ReacquireWoken,
    StartQueued,
    DispatchPolicy,
    DesktopSecond,
    SandboxSweep,
    Usage,
];

fn set(groups: &[&[StepId]]) -> BTreeSet<StepId> {
    groups.iter().flat_map(|g| g.iter().copied()).collect()
}

fn quiet(p: &mut Planner<'_>) {
    observe(p, &[], &[]);
}

fn new_pane(p: &mut Planner<'_>) {
    observe(p, &[], &[pane("p1")]);
}

fn held(p: &mut Planner<'_>) {
    observe(p, &[], &[]);
    p.observe_resources(&resources(true));
}

/// The global steps an incremental pass at `now` runs after a quiet warm-up.
fn case(
    config: &Config,
    now: Timestamp,
    feed: FeedDelta,
    git: GitChanges,
    launch_exits: bool,
    look: fn(&mut Planner<'_>),
) -> BTreeSet<StepId> {
    let mut memory = warm(config, &[], &[]);
    let mut planner = memory.plan(PassRequest::Paced, now, config, feed, git, launch_exits);
    look(&mut planner);
    fired(&mut planner)
}

fn feed_case(config: &Config, feed: FeedDelta) -> BTreeSet<StepId> {
    case(config, at(1), feed, GitChanges::default(), false, quiet)
}

fn quiet_case(config: &Config, now: Timestamp, git: GitChanges, launch: bool) -> BTreeSet<StepId> {
    case(config, now, FeedDelta::default(), git, launch, quiet)
}

#[test]
fn each_row_fires_on_its_tokens_and_not_otherwise() {
    let (_tmp, config) = config();
    let none = GitChanges::default;
    assert!(feed_case(&config, FeedDelta::default()).is_empty());
    // ANY alone: a write to an area no rule lists.
    let any = feed_areas("digest-schedule", "last");
    assert_eq!(feed_case(&config, any), set(&[&ANY_ROWS]));
    // A step's own cursor write is no input to the others (it would cost a second echo).
    assert!(feed_case(&config, feed_areas("lifecycle-gc", "board")).is_empty());
    let own = feed_case(&config, feed_areas("main-movement", "T-001"));
    assert_eq!(own, set(&[&[Movement]]));
    let tasks = FeedDelta {
        tasks: BTreeSet::from([TaskNum(1)]),
        ..FeedDelta::default()
    };
    assert_eq!(feed_case(&config, tasks), set(&[&ANY_ROWS, &TASK_ROWS]));
    let approved = FeedDelta {
        event_kinds: BTreeSet::from(["approved".to_string()]),
        newest_event_at: BTreeMap::from([("approved".to_string(), at(1))]),
        ..FeedDelta::default()
    };
    let expected = set(&[&ANY_ROWS, &[LessonsDue, Movement, PmEvents]]);
    assert_eq!(feed_case(&config, approved), expected);
    let expected = set(&[&ANY_ROWS, &[PmPolicy, PmEvents]]);
    assert_eq!(feed_case(&config, feed_areas("pm", "state")), expected);
    let expected = set(&[&ANY_ROWS, &[ResourcePhase, StartQueued, DispatchPolicy]]);
    assert_eq!(feed_case(&config, feed_areas("claims", "T-001")), expected);
    let scratch = FeedDelta {
        scratch: true,
        ..FeedDelta::default()
    };
    let expected = set(&[&ANY_ROWS, &[ScratchReclaim]]);
    assert_eq!(feed_case(&config, scratch), expected);
    let lifecycle = FeedDelta {
        lifecycle: true,
        ..FeedDelta::default()
    };
    assert_eq!(
        feed_case(&config, lifecycle),
        set(&[&ANY_ROWS, &[LifecycleGc]])
    );
    let expected = set(&[&[
        PmPolicy,
        ScratchReclaim,
        SweepInventory,
        OwnerDelivery,
        ReviewBudget,
        Availability,
        Attention,
        PmEvents,
    ]]);
    let fired = case(
        &config,
        at(1),
        FeedDelta::default(),
        none(),
        false,
        new_pane,
    );
    assert_eq!(fired, expected);
    let expected = set(&[&[
        ResourcePhase,
        ResumeGates,
        StartQueued,
        DispatchPolicy,
        Attention,
    ]]);
    let fired = case(&config, at(1), FeedDelta::default(), none(), false, held);
    assert_eq!(fired, expected);
    let repo = GitChanges {
        projects: BTreeSet::from(["app".to_string()]),
        ..GitChanges::default()
    };
    let expected = set(&[&[PostLand, StartQueued]]);
    assert_eq!(quiet_case(&config, at(1), repo, false), expected);
    let expected = set(&[&[SandboxSweep, ReviewBudget]]);
    assert_eq!(quiet_case(&config, at(1), none(), true), expected);
    let mut due = set(&[&[
        Digest,
        LessonsDue,
        OutputsSchedule,
        OwnerDelivery,
        PmInbox,
        AgentHooksPass,
        PostLand,
        GateQueue,
        Availability,
        PmEvents,
        Usage,
        ReviewBudget,
        Supervise,
    ]]);
    assert_eq!(quiet_case(&config, at(20), none(), false), due);
    due.extend([
        PmPolicy,
        ScratchReclaim,
        LifecycleGc,
        SweepInventory,
        HandoffRecovery,
        AutopilotWorkspace,
        StartQueued,
        Attention,
        MaintainDatabase,
    ]);
    assert_eq!(quiet_case(&config, at(60), none(), false), due);
}

#[test]
fn sticky_and_in_pass_rerun_once_after_acting() {
    let (_tmp, config) = config();
    let mut memory = warm(&config, &[], &[]);
    let mut planner = paced(&mut memory, &config, at(1), FeedDelta::default());
    observe(&mut planner, &[], &[]);
    assert!(!planner.run(ReacquireWoken));
    planner.finished(ResourcePhase, wrote());
    assert!(planner.run(ReacquireWoken) && planner.run(HoldSecond));
    planner.finish(at(1));

    let mut planner = paced(&mut memory, &config, at(2), FeedDelta::default());
    observe(&mut planner, &[], &[]);
    assert_eq!(fired(&mut planner), set(&[&[ResourcePhase]]));
    planner.finished(ResourcePhase, Effect::default());
    planner.finish(at(2));

    let mut planner = paced(&mut memory, &config, at(3), FeedDelta::default());
    observe(&mut planner, &[], &[]);
    assert!(fired(&mut planner).is_empty());
    assert!(planner.finish(at(3)).quiet);
}

#[test]
fn full_pass_reasons() {
    let (_tmp, config) = config();
    let full = |memory: &mut PassMemory, request, now, config: &Config, feed| {
        let planner = memory.plan(request, now, config, feed, GitChanges::default(), false);
        planner.finish(now).full
    };
    let mut fresh = PassMemory::new();
    let start = full(
        &mut fresh,
        PassRequest::Paced,
        t0(),
        &config,
        FeedDelta::default(),
    );
    assert_eq!(start, Some(FullReason::Start));
    let cases: [(PassRequest, Timestamp, FeedDelta, Option<FullReason>); 7] = [
        (PassRequest::Nudged, at(1), FeedDelta::default(), None),
        (
            PassRequest::Full(FullReason::Owner),
            at(1),
            FeedDelta::default(),
            Some(FullReason::Owner),
        ),
        (
            PassRequest::Paced,
            at(1),
            FeedDelta {
                overflow: true,
                ..FeedDelta::default()
            },
            Some(FullReason::FeedGap),
        ),
        (
            PassRequest::Paced,
            at(-1),
            FeedDelta::default(),
            Some(FullReason::ClockJump),
        ),
        (
            PassRequest::Paced,
            at(601),
            FeedDelta::default(),
            Some(FullReason::ClockJump),
        ),
        (
            PassRequest::Paced,
            at(1),
            feed_areas("session", "binding"),
            Some(FullReason::Session),
        ),
        (PassRequest::Paced, at(1), FeedDelta::default(), None),
    ];
    for (request, now, feed, expected) in cases {
        let mut memory = warm(&config, &[], &[]);
        assert_eq!(full(&mut memory, request, now, &config, feed), expected);
    }

    let mut memory = warm(&config, &[], &[]);
    let mut changed = config.clone();
    changed.sync.idle_after_secs += 1;
    let reason = full(
        &mut memory,
        PassRequest::Paced,
        at(1),
        &changed,
        FeedDelta::default(),
    );
    assert_eq!(reason, Some(FullReason::Config));

    let mut memory = warm(&config, &[], &[]);
    assert_eq!(
        full(
            &mut memory,
            PassRequest::Paced,
            at(300),
            &config,
            FeedDelta::default()
        ),
        None
    );
    let periodic = full(
        &mut memory,
        PassRequest::Paced,
        at(600),
        &config,
        FeedDelta::default(),
    );
    assert_eq!(periodic, Some(FullReason::Periodic));

    let mut memory = warm(&config, &[], &[]);
    memory.set_audit(true);
    let audit = full(
        &mut memory,
        PassRequest::Paced,
        at(1),
        &config,
        FeedDelta::default(),
    );
    assert_eq!(audit, Some(FullReason::Audit));

    // herdr availability flipping mid-pass makes the rest of the pass full.
    let mut memory = warm(&config, &[], &[]);
    let mut planner = paced(&mut memory, &config, at(1), FeedDelta::default());
    assert!(!planner.run(HoldFirst));
    planner.observe_herdr(false);
    assert!(planner.run(HoldSecond));
    assert_eq!(planner.finish(at(1)).full, Some(FullReason::Herdr));
}

#[test]
fn next_due_is_the_earliest_future_step_or_task_due() {
    let (_tmp, config) = config();
    let mut memory = warm(&config, &[], &[]);
    let mut planner = paced(&mut memory, &config, at(1), FeedDelta::default());
    observe(&mut planner, &[], &[]);
    assert!(fired(&mut planner).is_empty());
    // Supervise ran at t0 every 15 s; nothing else is due sooner.
    assert_eq!(planner.finish(at(1)).next_due, Some(at(15)));

    // Movement acted: its quiet window is due 3 s after the pass that wrote.
    let tasks = FeedDelta {
        tasks: BTreeSet::from([TaskNum(1)]),
        ..FeedDelta::default()
    };
    let mut planner = paced(&mut memory, &config, at(2), tasks);
    observe(&mut planner, &[], &[]);
    assert!(planner.run(Movement));
    planner.finished(Movement, wrote());
    assert_eq!(planner.finish(at(2)).next_due, Some(at(5)));

    // The dispatch policy is due at the next UTC day boundary.
    let midnight: Timestamp = "2026-03-03T00:00:00Z".parse().unwrap();
    let rule = rule(DispatchPolicy);
    assert_eq!(step_due(&memory, DispatchPolicy, &rule), Some(midnight));
}

fn store_heads(n: usize) -> (tempfile::TempDir, Store, Vec<TaskHead>) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let heads = (0..n)
        .map(|i| {
            let task = store
                .create(&format!("T{i}"), None, Status::Ready, "")
                .unwrap();
            store.head(&task.id).unwrap().detach()
        })
        .collect();
    (tmp, store, heads)
}

fn running(head: &mut TaskHead, run: serde_json::Value) {
    let mut doc = json!({
        "id": "r1", "agent": "pi", "worktree": "", "branch": "b", "base": "main",
        "base_sha": "0", "started": at(-3600).to_string(), "seen_working": true,
    });
    if let (Some(doc), Some(extra)) = (doc.as_object_mut(), run.as_object()) {
        doc.extend(extra.clone());
    }
    head.task.status = Status::Running;
    head.current_run = Some(Box::new(serde_json::from_value::<RunHead>(doc).unwrap()));
}

/// Which of `heads` the per-task rule runs in one incremental pass.
fn dirty(
    memory: &mut PassMemory,
    store: &Store,
    heads: &[TaskHead],
    now: Timestamp,
    feed: FeedDelta,
) -> Vec<bool> {
    let mut planner = paced(memory, &store.config, now, feed);
    observe(&mut planner, heads, &[pane("p1")]);
    let ran = heads.iter().map(|h| planner.task(HeadLoop, h)).collect();
    planner.finish(now);
    ran
}

#[test]
fn pm_host_writes_mark_only_the_tasks_that_read_them() {
    let (_tmp, store, mut heads) = store_heads(2);
    heads[0].last_plan_run = Some(Box::new(
        serde_json::from_value(json!({
            "id": "P1", "profile": "p", "harness": "pi", "spec_revision": "r",
            "started": t0().to_string(), "output_dir": "/tmp", "rounds": 0,
            "state": "submitted",
        }))
        .unwrap(),
    ));
    let warmed = || warm(&store.config, &heads, &[pane("p1")]);
    for area in [
        "pm-usage-guard",
        "pm-supervisor",
        "pm-wake-keys-wait",
        "agent-wait-deadlines",
    ] {
        let feed = || feed_areas(area, "current");
        let marked = dirty(&mut warmed(), &store, &heads, at(1), feed());
        assert_eq!(marked, [false, false], "{area}");
        let fired = feed_case(&store.config, feed());
        assert!(fired.is_superset(&set(&[&ANY_ROWS])), "{area}");
    }
    // The PM binding is an input of the plan task only.
    let marked = dirty(
        &mut warmed(),
        &store,
        &heads,
        at(1),
        feed_areas("pm", "state"),
    );
    assert_eq!(marked, [true, false]);
}

#[test]
fn per_task_rule_runs_only_changed_or_due_tasks() {
    let (_tmp, store, heads) = store_heads(2);
    let (a, b) = (&heads[0], &heads[1]);
    let warmed = || warm(&store.config, &heads, &[pane("p1")]);
    let cases = [
        (FeedDelta::default(), [false, false]),
        (
            FeedDelta {
                tasks: BTreeSet::from([a.num]),
                ..FeedDelta::default()
            },
            [true, false],
        ),
        (
            FeedDelta {
                task_ids: BTreeSet::from([b.id.clone()]),
                ..FeedDelta::default()
            },
            [false, true],
        ),
        (
            feed_areas("autopilot", &format!("autopilot-{}", a.id)),
            [true, false],
        ),
        (feed_areas("claims", "global"), [true, true]),
        (feed_areas("unlisted-area", "state"), [true, true]),
        (feed_areas("pm-events", "cursor"), [false, false]),
        (feed_areas("main-movement", &a.id), [false, false]),
    ];
    for (feed, expected) in cases {
        let mut memory = warmed();
        assert_eq!(dirty(&mut memory, &store, &heads, at(1), feed), expected);
    }

    // A task that acted runs once more, then settles.
    let mut memory = warmed();
    let feed = FeedDelta {
        tasks: BTreeSet::from([a.num]),
        ..FeedDelta::default()
    };
    let mut planner = paced(&mut memory, &store.config, at(1), feed);
    observe(&mut planner, &heads, &[pane("p1")]);
    assert!(planner.task(HeadLoop, a));
    let changed = Effect {
        changes: 1,
        ..Effect::default()
    };
    planner.finished_task(HeadLoop, &a.id, changed);
    planner.finish(at(1));
    let empty = FeedDelta::default;
    assert_eq!(
        dirty(&mut memory, &store, &heads, at(2), empty()),
        [true, false]
    );
    assert_eq!(
        dirty(&mut memory, &store, &heads, at(3), empty()),
        [false, false]
    );

    // Review autopilot rechecks each task every 300 s.
    let mut memory = warmed();
    let mut planner = paced(&mut memory, &store.config, at(300), FeedDelta::default());
    observe(&mut planner, &heads, &[pane("p1")]);
    assert!(planner.task(Autopilot, a));
    assert!(!planner.task(HeadLoop, b));
}

#[test]
fn task_written_by_an_earlier_step_reruns_later_per_task_steps() {
    let (_tmp, store, heads) = store_heads(2);
    let (a, b) = (&heads[0], &heads[1]);
    let mut memory = warm(&store.config, &heads, &[pane("p1")]);
    let mut planner = paced(&mut memory, &store.config, at(1), FeedDelta::default());
    observe(&mut planner, &heads, &[pane("p1")]);
    assert!(!planner.task(PlanPilot, a));
    // A global step writes task a during the pass.
    let effect = Effect {
        tasks: BTreeSet::from([a.id.clone()]),
        ..wrote()
    };
    planner.finished(ResumeGates, effect);
    assert!(planner.task(PlanPilot, a) && planner.task(Autopilot, a));
    assert!(!planner.task(PlanPilot, b) && !planner.task(Autopilot, b));
    planner.finish(at(1));
}

#[test]
fn missing_live_pane_and_idle_threshold_make_a_task_due() {
    let (_tmp, store, mut heads) = store_heads(1);
    let since = at(-110).to_string();
    running(
        &mut heads[0],
        json!({"herdr": {"workspace_id": "w", "pane_id": "p1"}, "idle_since": since}),
    );
    let due = task_due(&heads[0], &store.config);
    // idle_after_secs 120 from 110 s ago, before the 20 s cadence.
    assert_eq!(due.next(Some(t0())), Some(at(10)));
    assert_eq!(due.next(Some(at(10))), Some(at(30)));
    assert_eq!(due.next(None), Some(Timestamp::MIN));

    let mut memory = warm(&store.config, &heads, &[pane("p1")]);
    assert_eq!(
        dirty(&mut memory, &store, &heads, at(1), FeedDelta::default()),
        [false]
    );
    // The pane vanished: the second sample must run although the inventory is unchanged.
    let mut planner = paced(&mut memory, &store.config, at(2), FeedDelta::default());
    observe(&mut planner, &heads, &[]);
    assert!(planner.task(HeadLoop, &heads[0]));
    planner.finish(at(2));
    let mut planner = paced(&mut memory, &store.config, at(3), FeedDelta::default());
    observe(&mut planner, &heads, &[]);
    assert!(planner.task(HeadLoop, &heads[0]));
    assert_eq!(planner.finish(at(3)).next_due, Some(at(10)));
    assert_eq!(
        dirty(&mut memory, &store, &heads, at(10), FeedDelta::default()),
        [true]
    );
}

#[test]
fn audit_reports_rule_that_omits_an_input() {
    let (_tmp, config) = config();
    let audit = |rules: fn(StepId) -> Rule| {
        let mut memory = warm(&config, &[], &[]);
        memory.rules = rules;
        memory.set_audit(true);
        let mut planner = paced(
            &mut memory,
            &config,
            at(1),
            feed_areas("digest-schedule", "x"),
        );
        observe(&mut planner, &[], &[]);
        assert!(planner.run(PmInbox));
        planner.finished(PmInbox, wrote());
        let report = planner.finish(at(1));
        assert_eq!(report.full, Some(FullReason::Audit));
        report.wrong_skips
    };
    assert!(audit(rule).is_empty());
    // Without its feed input and its cadence, nothing explains the action.
    let omits_feed: fn(StepId) -> Rule = |step| match step {
        PmInbox => Rule {
            any: false,
            every: None,
            ..rule(step)
        },
        step => rule(step),
    };
    assert_eq!(audit(omits_feed), vec![PmInbox]);
}

#[test]
fn audit_tells_races_and_cadences_from_a_missing_input() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(tmp.path()).unwrap());
    let mut memory = warm(&ctx.store.config, &[], &[]);
    memory.rules = |step| match step {
        LessonsDue => Rule {
            every: None,
            ..rule(step)
        },
        step => rule(step),
    };
    memory.set_audit(true);
    // The first audited pass catches the cursor up with the store's initial feed.
    memory
        .begin(&ctx, PassRequest::Paced, at(1))
        .unwrap()
        .finish(at(1));
    let mut planner = memory.begin(&ctx, PassRequest::Paced, at(2)).unwrap();
    // When a task is written after the pass sampled its inputs, before steps are decided.
    ctx.store.create("Raced", None, Status::Ready, "").unwrap();
    for step in [MigrateBacklog, Digest, LessonsDue] {
        assert!(planner.run(step));
        planner.finished(step, wrote());
    }
    let report = planner.finish(at(2));
    // Then a rule listing the task change raced, one whose 20 s cadence is not yet due is
    // delayed, and one with neither is a wrong skip.
    assert_eq!(report.full, Some(FullReason::Audit));
    assert_eq!(report.raced, vec![MigrateBacklog]);
    assert_eq!(report.delayed, vec![Digest]);
    assert_eq!(report.wrong_skips, vec![LessonsDue]);
}

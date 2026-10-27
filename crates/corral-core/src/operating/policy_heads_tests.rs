use super::*;
use crate::work::{self, Counter, HotScope, WorkSnapshot};
use crate::{ApprovedScope, PlanDeliveryTarget, Run, Verdict};
use jiff::SignedDuration;
use serde_json::json;

#[path = "oracle_tests.rs"]
mod oracle;
#[path = "publication_heads_tests.rs"]
mod publication;

fn clock() -> Timestamp {
    "2026-10-27T00:00:00Z".parse().unwrap()
}

struct Seed(u64);
impl Seed {
    fn pick(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.0 >> 32) % n
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    config: Config,
    tasks: Vec<Task>,
}

fn reviewed(task: &Task) -> crate::PlanRecord {
    serde_json::from_value(json!({
        "id": "P1",
        "artifact": {
            "run":"D1", "kind":"plan", "title":"Plan", "path":"plan.md",
            "source":"plan.md", "sha256":"a".repeat(64), "bytes":1, "added":clock()
        },
        "spec_revision":plan_contract_revision(task).unwrap(),
        "document": {
            "goal":"Deliver work", "affected_paths":["src/"],
            "non_goals":[], "decisions":[], "contracts":[], "owner_decisions":[],
            "dependencies":[{"task":"scope", "write_scope":["src/"], "depends_on":[], "acceptance":[]}],
            "acceptance":[{"scenario":"Works", "expected":"Verified"}],
            "verification":["cargo test"]
        },
        "reviews":[{"id":"PR1","reviewer":"omo","policy_digest":"policy",
            "verdict":"approve","at":clock()}],
        "run":null
    })).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        for project in ["app", "other", "outside"] {
            fs::create_dir_all(root.path().join(project).join(".git")).unwrap();
            store
                .edit_config(|doc| doc.set_str(&format!("projects.{project}.path"), project))
                .unwrap();
        }
        let config = store.operating_config().unwrap();
        Self {
            _root: root,
            store,
            config,
            tasks: Vec::new(),
        }
    }

    fn add(&mut self, project: &str, status: Status, second: i64) -> String {
        // Import the creation time with the initial header, whose creation column is immutable.
        let mut task: Task = serde_json::from_value(json!({
            "id":format!("T-{:03}", self.tasks.len() + 1),
            "title":"Work", "project":project, "status":status,
            "created":clock() + SignedDuration::from_secs(second), "updated":clock()
        }))
        .unwrap();
        task.body = "Body\n".into();
        self.store.fixture_put_task(&task).unwrap();
        let id = task.id.clone();
        self.tasks.push(task);
        id
    }

    fn put(&self, task: &Task) {
        self.store.fixture_put_task(task).unwrap();
    }

    fn grant(&mut self, cap: Option<u32>, confirmed: bool) {
        self.config.automation.mode = OperatingMode::Autopilot;
        self.config.automation.authorization = Some("grant".into());
        self.config.automation.revision = 1;
        self.config.automation.authority = OperatingAuthority {
            dispatch: true,
            accept: true,
            plan_accept: true,
            aggregate_accept: true,
        };
        self.config.automation.scope.max_new_tasks = cap;
        for project in ["app", "other"] {
            let (repository, repository_identity) =
                self.store.project_identity(&self.config, project).unwrap();
            self.config.automation.scope.projects.insert(
                project.into(),
                ScopedProject {
                    repository,
                    repository_identity,
                },
            );
        }
        let approval = confirmed.then(|| OperatingApproval {
            authorization: "grant".into(),
            revision: 1,
            workspace: self.store.root().into(),
            digest: self.config.automation.grant_digest().unwrap(),
            confirmed_by: "owner".into(),
            confirmed_at: clock(),
        });
        self.store
            .save_operating_state(&OperatingState {
                approval,
                ..Default::default()
            })
            .unwrap();
    }

    fn parity(&self, seed: u64) {
        let snapshot = self.store.read_only_snapshot().unwrap();
        let expected = oracle::Oracle(&snapshot)
            .resolve_operating_policy(&self.config)
            .unwrap();
        let actual = snapshot.resolve_operating_policy(&self.config).unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "seed {seed}"
        );
    }
}

#[test]
fn operating_policy_heads_match_seeded_full_task_oracle() {
    // Given boards with interleaved timestamps, all statuses, grants and malformed relations.
    for seed in 0..64 {
        let mut random = Seed(seed);
        let mut fx = Fixture::new();
        for _ in 0..24 {
            let project = ["app", "other", "outside"][usize::try_from(random.pick(3)).unwrap()];
            let status = [
                Status::Backlog,
                Status::Ready,
                Status::Done,
                Status::Dropped,
                Status::NeedsMe,
                Status::Idea,
            ][usize::try_from(random.pick(6)).unwrap()];
            fx.add(project, status, i64::try_from(random.pick(9)).unwrap() - 4);
        }
        // Include both a reciprocal Plan and a legacy tracker before introducing random defects.
        for parent_index in [0, 4] {
            let mut parent = fx.tasks[parent_index].clone();
            parent.tracker = true;
            parent.children = vec![
                fx.tasks[parent_index + 1].id.clone(),
                fx.tasks[parent_index + 2].id.clone(),
            ];
            if parent_index == 0 {
                parent.approved_scope_revision = random.pick(3);
                parent.approved_scope = Some(ApprovedScope {
                    goal: "Goal".into(),
                    acceptance: Vec::new(),
                    delivery_target: PlanDeliveryTarget::Main,
                    approved_design: None,
                });
            }
            fx.put(&parent);
            fx.tasks[parent_index] = parent.clone();
            for child in &mut fx.tasks[parent_index + 1..parent_index + 3] {
                child.project.clone_from(&parent.project);
                child.parent = Some(parent.id.clone());
                fx.store.fixture_put_task(child).unwrap();
            }
        }
        for index in 0..fx.tasks.len() {
            if random.pick(4) == 0 {
                let member =
                    oracle::Oracle(&fx.store).operating_member(&fx.config, &fx.tasks[index]);
                if let Ok(member) = member {
                    fx.config
                        .automation
                        .scope
                        .tasks
                        .insert(fx.tasks[index].id.clone(), member);
                }
            }
            if random.pick(6) == 0 {
                fx.config
                    .automation
                    .scope
                    .exclusions
                    .insert(fx.tasks[index].id.clone());
            }
        }
        let mut design = fx.tasks[8].clone();
        design.plans.push(reviewed(&design));
        if random.pick(2) == 0 {
            design.plans[0].reviews[0].verdict = Some(Verdict::Changes);
        }
        fx.put(&design);
        let member = oracle::Oracle(&fx.store)
            .operating_member(&fx.config, &design)
            .unwrap();
        fx.config
            .automation
            .scope
            .tasks
            .insert(design.id.clone(), member);
        fx.config.automation.scope.plans.push(ScopedPlan {
            task: design.id.clone(),
            revision: "P1".into(),
            artifact_sha256: "a".repeat(64),
            scopes: BTreeMap::from([(
                "scope".into(),
                digest(&design.plans[0].document.dependencies[0]).unwrap(),
            )]),
        });
        // Change reciprocal ownership, contracts and missing references after freezing explicit members.
        match random.pick(5) {
            0 => {
                let mut child = fx.tasks[1].clone();
                child.parent = None;
                fx.put(&child);
            }
            1 => {
                let mut owner = fx.tasks[4].clone();
                owner.children.push(fx.tasks[1].id.clone());
                fx.put(&owner);
            }
            2 => {
                let mut owner = fx.tasks[0].clone();
                owner.children.push("T-999".into());
                fx.put(&owner);
            }
            3 => {
                let mut child = fx.tasks[2].clone();
                child.body.push_str("Changed\n");
                child.dispatch_hold = true;
                fx.put(&child);
            }
            _ => {}
        }
        let cap = (seed % 4 != 0).then(|| u32::try_from(random.pick(9)).unwrap());
        fx.grant(cap, seed % 7 != 0);
        fx.config.automation.paused = seed % 3 == 0;
        // Pause is not part of the grant digest.
        // When both algorithms resolve the same frozen view, then every serialized field matches.
        fx.parity(seed);
    }
}

#[test]
fn execution_plan_heads_match_full_oracle_on_reciprocal_nonreciprocal_and_legacy() {
    for case in 0..4 {
        // Given a direct reciprocal Plan, a broken backlink/owner, or a legacy tracker.
        let mut fx = Fixture::new();
        let parent = fx.add("app", Status::Backlog, -1);
        let child = fx.add("app", Status::Ready, -1);
        let mut plan = fx.tasks[0].clone();
        plan.tracker = true;
        plan.children = vec![child.clone()];
        if case != 3 {
            plan.approved_scope = Some(ApprovedScope {
                goal: "Goal".into(),
                acceptance: Vec::new(),
                delivery_target: PlanDeliveryTarget::Main,
                approved_design: None,
            });
        }
        fx.put(&plan);
        let mut task = fx.tasks[1].clone();
        if case != 1 {
            task.parent = Some(parent);
        }
        fx.put(&task);
        if case == 2 {
            let owner = fx.add("app", Status::Backlog, -1);
            let mut extra = fx.store.get(&owner).unwrap();
            extra.tracker = true;
            extra.children = vec![child];
            fx.put(&extra);
        }
        let view = fx.store.read_only_snapshot().unwrap();
        let expected = oracle::Oracle(&view)
            .execution_plan(&task)
            .map(|plan| plan.map(|plan| plan.id))
            .map_err(|error| format!("{error:#}"));
        // When the head form resolves membership, then the id or exact refusal is unchanged.
        let hot = HotScope::enter();
        let actual = view
            .execution_plan_head(&view.head(&task.id).unwrap())
            .map(|plan| plan.map(|plan| plan.id.clone()))
            .map_err(|error| format!("{error:#}"));
        assert_eq!(actual, expected, "case {case}");
        assert_eq!(work::snapshot().unattributed_hydrations(), 0);
        drop(hot);
    }
}

fn run(id: &str) -> Run {
    serde_json::from_value(json!({
        "id":id,"agent":"omo","worktree":"/not-existing","branch":"work",
        "base":"main","base_sha":"base","started":clock(),"ended":clock(),
        "outcome":"done"
    }))
    .unwrap()
}

#[test]
fn operating_policy_heads_preserve_pending_decision_revision_refusals() {
    for resolved in [false, true] {
        // Given two pending questions and owner Changes notes answering one or both.
        let mut fx = Fixture::new();
        fx.add("app", Status::Ready, -1);
        let mut task = fx.tasks[0].clone();
        let mut current = run("R1");
        current.ended = None;
        for question in ["First", "Second"] {
            current.obligations.decisions.push(serde_json::from_value(json!({
                "request":{"version":1,"id":question,"run":"R1","question":question,
                    "requested_action":"answer","owner":"owner","source":"corral","deadline":null},
                "registered_at":clock(),
                "observation":{"at":clock(),"state":"pending","detail":""},
                "receipt":null
            })).unwrap());
        }
        task.runs.push(current);
        let mut plan = reviewed(&task);
        plan.reviews[0].reviewer = "owner".into();
        plan.reviews[0].verdict = Some(Verdict::Changes);
        plan.reviews[0].notes = Some(
            if resolved {
                "Resolved: First => Yes\nResolved: Second => Yes"
            } else {
                "Resolved: First => Yes"
            }
            .into(),
        );
        task.plans.push(plan);
        fx.put(&task);
        fx.grant(None, true);
        // When pending pages feed the head resolver, then both common and per-action reasons match.
        fx.parity(u64::from(resolved));
        measure(&fx);
    }
}

fn grow_history(task: &mut Task, count: u32) {
    task.runs = (1..=count).map(|n| run(&format!("R{n:06}"))).collect();
    task.runs.push(run("R999999"));
    task.plan_runs = (1..=count)
        .map(|n| {
            serde_json::from_value(json!({
                "id":format!("D{n:06}"), "profile":"omo", "harness":"generic",
                "spec_revision":"spec", "started":clock(), "ended":clock(),
                "output_dir":"/not-existing", "rounds":0, "state":"approved"
            }))
            .unwrap()
        })
        .collect();
    let mut current = task.plan_runs[0].clone();
    current.id = "D999999".into();
    task.plan_runs.push(current);
    task.comments = (0..count)
        .map(|_| crate::TaskComment {
            author: crate::CommentAuthor::Owner,
            at: clock(),
            text: "History".into(),
        })
        .collect();
    let artifact = reviewed(task).artifact;
    task.artifacts = vec![artifact; usize::try_from(count).unwrap()];
}

fn measure(fx: &Fixture) -> (serde_json::Value, WorkSnapshot) {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let policy = fx.store.resolve_operating_policy(&fx.config).unwrap();
    let counts = work::snapshot().since(&before);
    counts.check_identity().unwrap();
    assert_eq!(counts.unattributed_hydrations(), 0);
    (serde_json::to_value(policy).unwrap(), counts)
}

#[test]
fn operating_policy_heads_ignore_tenfold_terminal_and_history_growth() {
    for (cap, confirmed) in [
        (Some(2), true),
        (Some(0), true),
        (None, true),
        (Some(2), false),
    ] {
        budget_case(cap, confirmed);
    }
}

fn budget_case(cap: Option<u32>, confirmed: bool) {
    // Given open and capped terminal members in both projects, with fixed current inputs.
    let mut fx = Fixture::new();
    for (project, status, second) in [
        ("app", Status::Ready, -4),
        ("other", Status::Backlog, -3),
        ("app", Status::Done, 1),
        ("other", Status::Dropped, 2),
        ("app", Status::Done, 3),
        ("other", Status::Done, 4),
    ] {
        fx.add(project, status, second);
    }
    for task in &mut fx.tasks {
        grow_history(task, 1);
        fx.store.fixture_put_task(task).unwrap();
        fx.store
            .log_event(Some(&task.id), "history", json!({}))
            .unwrap();
    }
    fx.grant(cap, confirmed);
    let base = measure(&fx);
    // When every task's history and the terminal population grow tenfold.
    for task in &mut fx.tasks {
        grow_history(task, 10);
        fx.store.fixture_put_task(task).unwrap();
        for _ in 0..9 {
            fx.store
                .log_event(Some(&task.id), "history", json!({}))
                .unwrap();
        }
    }
    for index in 0..36 {
        let id = fx.add(
            if index % 2 == 0 { "app" } else { "other" },
            Status::Done,
            10 + index,
        );
        let mut task = fx.store.get(&id).unwrap();
        grow_history(&mut task, 10);
        fx.put(&task);
        for _ in 0..10 {
            fx.store.log_event(Some(&id), "history", json!({})).unwrap();
        }
    }
    let grown = measure(&fx);
    // Then no listing/hydration occurs and every unattributed counter, including bytes, is equal.
    assert_eq!(base.0, grown.0);
    for counter in Counter::ALL {
        assert_eq!(
            base.1.get(counter),
            grown.1.get(counter),
            "{}",
            counter.name()
        );
    }
    assert_eq!(
        base.1.unattributed_byte_remainder(),
        grown.1.unattributed_byte_remainder()
    );
}

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use corral_core::backend::TaskParts;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{Lesson, LessonScope, ResourceState, Run, Status, Store, Task};
use corral_ops::Ctx;
use corral_ops::resources::PinnedReadingsProbe;
use serde_json::json;

pub const FU: &str = "FU-3";
pub const NOW: &str = "2026-10-27T00:00:00Z";

pub struct Fixture {
    pub _tmp: tempfile::TempDir,
    pub ctx: Ctx,
    pub probe: Arc<PinnedReadingsProbe>,
    pub idea: Task,
    pub active: Task,
    pub tasks: Vec<Task>,
    pub fixed_paths: BTreeSet<PathBuf>,
    pub paths: BTreeSet<PathBuf>,
}

impl Fixture {
    pub fn new(scale: usize, similar: bool, existing: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        for name in ["app", "other"] {
            std::fs::create_dir_all(tmp.path().join(name).join(".git")).unwrap();
            store.config.projects.insert(
                name.into(),
                serde_json::from_value(json!({"path":name})).unwrap(),
            );
        }
        store.config.limits.running = 10;
        store.config.dispatch.promote_ideas = corral_core::PromoteIdeas::Suggest;
        store.config.storage.cache_root = Some(tmp.path().join("cache"));
        store.config.pm.push_events = false;
        store.config.validate(store.root()).unwrap();
        assert_eq!(store.config.resources, corral_core::ResourcesCfg::default());
        store.save_config().unwrap();
        let idea = store
            .create(
                "Copper lattice",
                Some("app"),
                Status::Idea,
                "## Acceptance\n- Verify the result.",
            )
            .unwrap();
        let active = store
            .create("Quartz nozzle", Some("app"), Status::Running, "")
            .unwrap();
        store
            .update(&active.id, |t| {
                t.write_scope = vec!["unrelated/".into()];
                Ok(())
            })
            .unwrap();
        for index in 0..scale {
            let title = if similar {
                format!("Copper lattice {index}")
            } else {
                format!("Zirconium basalt {index}")
            };
            store.create(&title, Some("app"), Status::Done, "").unwrap();
        }
        // An unrelated project exercises project filtering and fixed volume inputs.
        for index in 0..scale {
            store
                .create(
                    &format!("Saffron turbine {index}"),
                    Some("other"),
                    Status::Dropped,
                    "",
                )
                .unwrap();
        }
        let initial = store.list().unwrap();
        for task in initial {
            let diff = format!("artifacts/{}/change.diff", task.id);
            std::fs::create_dir_all(store.dir().join("artifacts").join(&task.id)).unwrap();
            std::fs::write(
                store.dir().join(&diff),
                "diff --git a/src/copper.rs b/src/copper.rs\n--- a/src/copper.rs\n+++ b/src/copper.rs\n",
            ).unwrap();
            store
                .update(&task.id, |t| {
                    // Keep current inputs fixed; only earlier runs and comments grow.
                    if t.id != idea.id {
                        for index in 0..scale {
                            let path = store
                                .root()
                                .join("history")
                                .join(&t.id)
                                .join(format!("{index:03}"));
                            if existing {
                                std::fs::create_dir_all(&path).unwrap();
                            }
                            t.runs.push(run(format!("H{index:03}"), path));
                        }
                        t.runs.push(run(
                            "R-current".into(),
                            store.root().join("absent").join(&t.id),
                        ));
                    }
                    for index in 0..scale {
                        t.comments.push(corral_core::TaskComment {
                            at: NOW.parse()?,
                            author: corral_core::CommentAuthor::Pm,
                            text: format!("history {index}"),
                        });
                        if t.id != idea.id {
                            t.plan_runs.push(serde_json::from_value(json!({
                                "id":format!("P{index:03}"),"profile":"generic","harness":"generic",
                                "spec_revision":"historic","started":NOW,"ended":NOW,
                                "output_dir":store.root().join("plan-output"),
                                "rounds":0,"state":"cancelled"
                            }))?);
                        }
                        t.artifacts.push(serde_json::from_value(json!({
                            "path":diff,"kind":"diff","title":format!("Touched files {index}"),"added":NOW,
                            "run":"R-current","source":diff,"sha256":"0".repeat(64),"bytes":92
                        }))?);
                    }
                    Ok(())
                })
                .unwrap();
            for index in 0..scale {
                store
                    .log_event(
                        Some(&task.id),
                        "fixture_telemetry",
                        json!({"history":index}),
                    )
                    .unwrap();
            }
        }
        let lesson = Lesson {
            id: "L-copper".into(),
            rule: "Check the copper path".into(),
            why: "Historical evidence".into(),
            evidence: vec![],
            scope: LessonScope {
                files: vec!["src/copper.rs".into()],
                ..Default::default()
            },
            confidence: 1.0,
            pinned: true,
            contradicted: false,
            last_seen: NOW.parse().unwrap(),
        };
        store.save_lessons("app", &[lesson]).unwrap();
        let tasks = store.list().unwrap();
        let idea = store.get(&idea.id).unwrap();
        let active = store.get(&active.id).unwrap();
        let fixed_paths = fixed_paths(&store);
        let mut paths = fixed_paths.clone();
        for task in &tasks {
            for run in &task.runs {
                if run.worktree.exists() {
                    paths.insert(run.worktree.clone());
                    paths.insert(run.worktree.join("target"));
                }
            }
        }
        let probe = Arc::new(PinnedReadingsProbe::admitting());
        let ctx = Ctx {
            resource_probe: probe.clone(),
            // No test can accidentally reach a live engine.
            herdr: corral_herdr::Herdr::with_bin(tmp.path().join("unused-herdr"), None),
            store,
        };
        Self {
            _tmp: tmp,
            ctx,
            probe,
            idea,
            active,
            tasks,
            fixed_paths,
            paths,
        }
    }

    pub fn state_preconditions(&self, state: &ResourceState) {
        assert!(!state.held && !state.severe && state.reason.is_none());
        assert_eq!(
            state
                .snapshot
                .disks
                .iter()
                .map(|d| d.path.clone())
                .collect::<BTreeSet<_>>(),
            self.paths,
        );
        assert_eq!(state.snapshot.load_one, 0.0);
        assert_eq!(state.snapshot.load_per_core, 0.0);
        assert_eq!(state.snapshot.mem_available, u64::MAX);
        assert!(
            state
                .snapshot
                .disks
                .iter()
                .all(|d| d.free_bytes == u64::MAX)
        );
    }

    /// Raw stored documents, independently cross-checked against isolated full reads.
    pub fn hydration_work(&self) -> WorkSnapshot {
        let store = self.ctx.store.read_only_snapshot().unwrap();
        let view = store.view().unwrap();
        let mut rows = 0;
        let mut bytes = 0;
        for task in &self.tasks {
            let num = store.head(&task.id).unwrap().num;
            let doc = view.task(num, &TaskParts::All).unwrap().unwrap();
            rows += 1 + u64::try_from(doc.rows.len()).unwrap();
            bytes += size(&doc.head.doc) + size(&doc.head.summary);
            bytes += doc.rows.iter().map(|r| size(&r.doc)).sum::<u64>();
        }
        let (_, cells) = measure(|| {
            for task in &self.tasks {
                store.get(&task.id).unwrap();
            }
        });
        assert_eq!(
            Counter::DECODES
                .iter()
                .map(|c| cells.total(*c))
                .sum::<u64>(),
            rows
        );
        assert_eq!(cells.decoded_doc_bytes(), bytes);
        assert_eq!(
            cells.full_task_hydrations(),
            u64::try_from(self.tasks.len()).unwrap()
        );
        cells
    }

    pub fn sample_stats(&self) -> u64 {
        let probes = u64::try_from(self.tasks.iter().map(|t| t.runs.len()).sum::<usize>()).unwrap();
        probes
            + self
                .paths
                .difference(&self.fixed_paths)
                .map(|path| path_stats(path))
                .sum::<u64>()
    }
}

pub fn run(id: String, worktree: PathBuf) -> Run {
    serde_json::from_value(json!({
        "id":id,"agent":"generic","harness":"generic","worktree":worktree,
        "branch":"corral/fixture","base":"main","base_sha":"0".repeat(40),
        "started":NOW,"ended":NOW,"outcome":"done"
    }))
    .unwrap()
}

fn size(value: &serde_json::Value) -> u64 {
    u64::try_from(serde_json::to_vec(value).unwrap().len()).unwrap()
}

pub fn fixed_paths(store: &Store) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::from([store.dir().to_path_buf()]);
    for name in store.config.projects.keys() {
        let project = store.project_path(name).unwrap();
        paths.insert(project.join(".worktrees"));
        paths.insert(project.join("target"));
    }
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        let target = PathBuf::from(target);
        paths.insert(if target.is_absolute() {
            target
        } else {
            std::env::current_dir().unwrap().join(target)
        });
    }
    paths
}

pub fn path_stats(path: &std::path::Path) -> u64 {
    let mut probes = 0;
    for parent in path.ancestors() {
        probes += 1;
        if parent.exists() {
            return probes + 1;
        }
    }
    panic!("fixture path has no existing ancestor: {}", path.display());
}

pub fn measure<T>(call: impl FnOnce() -> T) -> (T, WorkSnapshot) {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    before.check_identity().unwrap();
    let value = call();
    let after = work::snapshot();
    after.check_identity().unwrap();
    let delta = after.since(&before);
    delta.check_identity().unwrap();
    (value, delta)
}

pub fn assert_sample(cells: &WorkSnapshot, fixture: &Fixture, calls: u64, bound: bool) {
    assert_eq!(fixture.probe.samples(), calls);
    assert_eq!(cells.followup_calls(FU, "sample"), calls);
    let hydrated = fixture.hydration_work();
    assert_eq!(
        cells.followup_hydrations(FU, "sample"),
        calls * hydrated.full_task_hydrations()
    );
    assert_eq!(
        cells.followup_rows(FU, "sample"),
        calls
            * Counter::DECODES
                .iter()
                .map(|c| hydrated.total(*c))
                .sum::<u64>()
    );
    assert_eq!(
        cells.followup_doc_bytes(FU, "sample"),
        calls * hydrated.decoded_doc_bytes()
    );
    assert_eq!(
        cells.followup_views(FU, "sample"),
        if bound { 0 } else { calls }
    );
    assert_eq!(
        cells.followup_stats(FU, "sample"),
        calls * fixture.sample_stats()
    );
    assert_eq!(cells.followup_processes(FU, "sample"), 0);
}

pub fn assert_growth(base: &WorkSnapshot, grown: &WorkSnapshot) {
    for counter in Counter::ALL {
        let left = base.cells(counter);
        let right = grown.cells(counter);
        assert_eq!(left.unattributed, right.unattributed, "{counter:?}");
        assert_eq!(left.own_record, right.own_record, "{counter:?}");
        assert_eq!(left.emitted, right.emitted, "{counter:?}");
        assert_eq!(
            i128::from(right.total) - i128::from(left.total),
            i128::from(right.followup_sum()) - i128::from(left.followup_sum()),
            "{counter:?}"
        );
    }
    assert_eq!(base.unattributed_hydrations(), 0);
    assert_eq!(grown.unattributed_hydrations(), 0);
}

use std::path::{Path, PathBuf};

use corral_core::{
    ArtifactKind, CheckpointUpdate, CriteriaManifest, Criterion, EvidenceObservation,
    ObservationStatus, ProgressPhase, ProgressStep, ProgressUpdate, ResourceObligation, Run,
    Status, StepState, Store, Task, WaitObligation,
};
use corral_ops::Ctx;
use serde_json::{Value, json};

pub fn git(dir: &Path, args: &[&str]) {
    let out = crate::support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub ctx: Ctx,
    pub id: String,
    pub repo: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "corral test"]);
        git(&repo, &["config", "user.email", "corral@test.invalid"]);
        std::fs::write(repo.join("README.md"), "initial\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.herdr.session = Some("corral-test".into());
        store.save_config().unwrap();
        let ctx = Ctx {
            herdr: corral_herdr::Herdr::with_bin(
                tmp.path().join("unused-herdr"),
                Some("corral-test".into()),
            ),
            store,
        };
        let task = ctx
            .store
            .create("Portable progress", Some("app"), Status::Running, "Spec.\n")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id": "R1",
            "agent": "fake",
            "worktree": repo,
            "branch": "main",
            "base": "main",
            "base_sha": corral_git::rev_sha(&repo, "HEAD").unwrap(),
            "started": "2026-10-15T00:00:00Z",
            "checkpoint_required": true
        }))
        .unwrap();
        ctx.store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        corral_ops::register_criteria(
            &ctx,
            &task.id,
            &CriteriaManifest {
                version: 1,
                spec_digest: corral_ops::completion_spec_digest(&task).unwrap(),
                reviewed_by: "owner".into(),
                audited: false,
                require_gate: false,
                criteria: vec![Criterion {
                    id: "C-1".into(),
                    required: true,
                    live: true,
                    scenario: "Portable progress".into(),
                    expected: "zero failures".into(),
                }],
                owner_decisions: vec![],
            },
        )
        .unwrap();
        Self {
            tmp,
            ctx,
            id: task.id,
            repo,
        }
    }

    pub fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    pub fn progress(&self) -> ProgressUpdate {
        let read = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        ProgressUpdate {
            task: read.task,
            run: read.run,
            spec_revision: read.spec_revision,
            expected_revision: read.progress.as_ref().map_or(0, |p| p.revision),
            phases: vec![
                ProgressPhase {
                    id: "design".into(),
                    label: "Design".into(),
                    steps: vec![ProgressStep {
                        id: "contract".into(),
                        label: "Read contract".into(),
                        status: StepState::Done,
                        criteria: vec!["C-1".into()],
                    }],
                },
                ProgressPhase {
                    id: "implementation".into(),
                    label: "Implementation".into(),
                    steps: vec![
                        ProgressStep {
                            id: "code".into(),
                            label: "Write code".into(),
                            status: StepState::InProgress,
                            criteria: vec!["C-1".into()],
                        },
                        ProgressStep {
                            id: "verify".into(),
                            label: "Verify".into(),
                            status: StepState::Pending,
                            criteria: vec!["C-1".into()],
                        },
                    ],
                },
            ],
        }
    }

    pub fn seed_progress(&self) {
        corral_ops::update_progress(&self.ctx, &self.id, &self.progress()).unwrap();
    }

    pub fn checkpoint(&self) -> CheckpointUpdate {
        let read = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        CheckpointUpdate {
            task: read.task,
            run: read.run,
            spec_revision: read.spec_revision,
            expected_revision: read.checkpoints.last().map_or(0, |c| c.checkpoint.revision),
            progress_revision: read.progress.as_ref().map_or(0, |p| p.revision),
            head: read.head,
            base: read.base,
            tree: read.tree,
            criterion_ids: vec!["C-1".into()],
            evidence_refs: vec![],
            next_action: "Finish verification".into(),
            decisions: vec!["Keep stable identities".into()],
            waits: vec![WaitObligation {
                id: "build".into(),
                owner: "R1".into(),
                handle: "build-42".into(),
                condition: "exit".into(),
                resolution: None,
            }],
            resources: vec![ResourceObligation {
                id: "server".into(),
                owner: "R1".into(),
                description: "isolated server".into(),
                cleanup: "stop server-42".into(),
                resolution: None,
            }],
        }
    }

    pub fn seed_checkpoint(&self) -> CheckpointUpdate {
        self.seed_progress();
        let source = self.tmp.path().join("checks.json");
        std::fs::write(&source, b"{\"exit_code\":0}\n").unwrap();
        let evidence = corral_ops::add_artifact(
            &self.ctx,
            &self.id,
            &source,
            Some(ArtifactKind::Evidence),
            None,
        )
        .unwrap();
        let mut input = self.checkpoint();
        let receipt = corral_ops::record_evidence(
            &self.ctx,
            &self.id,
            &EvidenceObservation {
                version: 1,
                key: "capture-1".into(),
                criterion: "C-1".into(),
                run: "R1".into(),
                attempt: "no-gate".into(),
                status: ObservationStatus::Pass,
                action: "Read fixture result".into(),
                environment: "corral-test".into(),
                expected: "zero failures".into(),
                observed: "zero failures".into(),
                artifacts: vec![evidence.path],
                // Progress tests need an owned capture, not a wall-clock ordering test.
                captured: self.task().current_run().unwrap().started,
                producer: "fixture".into(),
                live: true,
                cleanup: None,
            },
        )
        .unwrap();
        input.evidence_refs.push(receipt.observation.key);
        corral_ops::save_checkpoint(&self.ctx, &self.id, &input).unwrap();
        input
    }

    pub fn packet(&self) -> Value {
        let task = self.task();
        let prompt =
            corral_ops::task_prompt(&self.ctx, &task, task.current_run().unwrap()).unwrap();
        let data = prompt
            .split_once("<corral-resume>\n")
            .unwrap()
            .1
            .split_once("\n</corral-resume>")
            .unwrap()
            .0;
        serde_json::from_str(data).unwrap()
    }

    pub fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        fn collect(dir: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(&path, files);
                } else {
                    files.push((path.clone(), std::fs::read(path).unwrap()));
                }
            }
        }
        let mut files = vec![];
        // Locks are operational state, not task/event/artifact writes.
        for dir in ["tasks", "artifacts"] {
            collect(&self.ctx.store.dir().join(dir), &mut files);
        }
        let events = self.ctx.store.dir().join("events.jsonl");
        files.push((events.clone(), std::fs::read(events).unwrap()));
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }
}

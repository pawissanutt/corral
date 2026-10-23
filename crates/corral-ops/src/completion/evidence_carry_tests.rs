use std::path::Path;

use corral_core::{
    CriteriaManifest, Criterion, EvidenceObservation, EvidenceSource, GateRecord, GateResult,
    ObservationStatus, Run, Status,
};

use super::*;
use crate::completion::{completion_check_task, evidence_failures, record_evidence};

const SUMMARY: &str = "Verified: fixture\nNot verified: semantic review\n";
const METHODS: [CarryMethod; 3] = [
    CarryMethod::CleanReplay,
    CarryMethod::TierPromotion,
    CarryMethod::CleanReplayTierPromotion,
];

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
}

struct Fx {
    _dir: tempfile::TempDir,
    ctx: Ctx,
    id: String,
    artifact: String,
}

#[derive(Clone, Copy)]
struct Shape {
    audited: bool,
    required: bool,
}

const AUDITED: Option<Shape> = Some(Shape {
    audited: true,
    required: true,
});

impl Fx {
    /// Run R1 started 2026-01-01 on an untiered project, with one artifact snapshot.
    fn new(shape: Option<Shape>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir(&app).unwrap();
        git(&app, &["init", "-qb", "main"]);
        git(&app, &["config", "user.name", "Fixture"]);
        git(&app, &["config", "user.email", "fixture@example.invalid"]);
        git(&app, &["config", "commit.gpgsign", "false"]);
        std::fs::write(app.join("code.txt"), "initial\n").unwrap();
        corral_git::commit_all(&app, "initial").unwrap();
        let store = corral_core::Store::init(dir.path()).unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(store);
        let task = ctx
            .store
            .create("criterion", Some("app"), Status::Running, "spec")
            .unwrap();
        let base = corral_git::rev_sha(&app, "HEAD").unwrap();
        let run: Run = serde_json::from_value(serde_json::json!({
            "id": "R1", "agent": "generic", "worktree": app, "branch": "main",
            "base": "main", "base_sha": base, "started": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        ctx.store
            .update(&task.id, |t| {
                t.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
        let source = dir.path().join("proof.log");
        std::fs::write(&source, "fixture bytes\n").unwrap();
        let artifact = crate::add_artifact(&ctx, &task.id, &source, None, None)
            .unwrap()
            .path;
        let fx = Self {
            _dir: dir,
            ctx,
            id: task.id,
            artifact,
        };
        if let Some(shape) = shape {
            fx.register(shape);
        }
        fx
    }

    fn register(&self, shape: Shape) {
        let manifest = CriteriaManifest {
            version: 1,
            spec_digest: super::super::completion_spec_digest(&self.task()).unwrap(),
            reviewed_by: "owner".into(),
            audited: shape.audited,
            require_gate: false,
            criteria: vec![Criterion {
                id: "C1".into(),
                required: shape.required,
                live: true,
                scenario: "fixture result".into(),
                expected: "zero failures".into(),
            }],
            owner_decisions: vec![],
        };
        super::super::register_criteria(&self.ctx, &self.id, &manifest).unwrap();
    }

    fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    /// Record C1 with `status` against the current identity A, captured 2026-01-02.
    fn record(&self, status: ObservationStatus) -> CandidateIdentity {
        let observation = EvidenceObservation {
            version: 1,
            key: "capture-1".into(),
            criterion: "C1".into(),
            run: "R1".into(),
            attempt: "no-gate".into(),
            status,
            action: "fixture observation".into(),
            environment: "isolated".into(),
            expected: "zero failures".into(),
            observed: "zero failures".into(),
            artifacts: vec![self.artifact.clone()],
            captured: ts("2026-01-02T00:00:00Z"),
            producer: "generic worker".into(),
            live: true,
            cleanup: None,
        };
        record_evidence(&self.ctx, &self.id, &observation)
            .unwrap()
            .identity
    }

    /// Bind a passing gate receipt for `attempt`, stamped now, and return its identity.
    fn gate(&self, attempt: &str) -> (CandidateIdentity, Timestamp) {
        let task = self.task();
        let run = task.current_run().unwrap();
        let identity = verification::capture_identity(&self.ctx, &task, run, attempt).unwrap();
        let at = corral_core::clock::now();
        let receipt = identity.clone();
        self.mutate(|task| {
            task.current_run_mut().unwrap().gate = Some(GateRecord {
                sha: receipt.head.clone(),
                identity: Some(receipt),
                commands: vec![],
                log_sha256: None,
                result: GateResult::Pass,
                process_lost: false,
                failed_cmd: None,
                at,
                pane: None,
                socket: None,
                tier: None,
            });
        });
        (identity, at)
    }

    fn mutate(&self, f: impl FnOnce(&mut Task)) {
        let mut f = Some(f);
        self.ctx
            .store
            .update(&self.id, |task| {
                (f.take().unwrap())(task);
                Ok(())
            })
            .unwrap();
    }

    fn establish(
        &self,
        source: &CandidateIdentity,
        window_start: Timestamp,
        to: &CandidateIdentity,
        method: CarryMethod,
    ) -> Result<Option<EvidenceCarry>> {
        let task = self.task();
        let source = CarrySource {
            identity: source,
            window_start,
        };
        establish(
            &self.ctx,
            &task,
            task.current_run().unwrap(),
            source,
            to,
            method,
        )
    }

    fn store_carry(&self, carry: EvidenceCarry) {
        self.mutate(|task| {
            task.current_run_mut().unwrap().completion.evidence_carry = Some(Box::new(carry));
        });
    }

    fn failures(&self) -> Vec<String> {
        let task = self.task();
        evidence_failures(&self.ctx, &task, task.current_run().unwrap()).unwrap()
    }

    fn failures_without_carry(&self) -> Vec<String> {
        let mut task = self.task();
        task.current_run_mut().unwrap().completion.evidence_carry = None;
        evidence_failures(&self.ctx, &task, task.current_run().unwrap()).unwrap()
    }

    /// Record C1 at identity A, then gate attempt N1 and carry A's records to it.
    fn carried(&self, method: CarryMethod) -> (CandidateIdentity, CandidateIdentity) {
        let a = self.record(ObservationStatus::Pass);
        let (n, _) = self.gate("N1");
        let started = self.task().current_run().unwrap().started;
        let carry = self.establish(&a, started, &n, method).unwrap().unwrap();
        self.store_carry(carry);
        (a, n)
    }
}

fn refusal(result: Result<Option<EvidenceCarry>>) -> Vec<String> {
    result
        .unwrap_err()
        .downcast::<CarryRefused>()
        .unwrap()
        .failures
}

#[test]
fn live_carry_admits_records_bound_to_the_origin_without_rewriting_them() {
    for method in METHODS {
        // Given an audited run whose C1 record is bound to A and a fresh gate N.
        let fx = Fx::new(AUDITED);
        let a = fx.record(ObservationStatus::Pass);
        let (n, _) = fx.gate("N1");
        let before = fx.task();
        let run = before.current_run().unwrap();
        let ledger = serde_json::to_vec(&run.completion.evidence).unwrap();
        let digest = verification::evidence_digest(run).unwrap();
        assert!(fx.failures().contains(&"evidence_stale:C1".into()));

        // When the records are carried from A to N.
        let carry = fx.establish(&a, run.started, &n, method).unwrap().unwrap();
        assert_eq!((&carry.from, &carry.to), (&a, &n));
        assert_eq!((carry.window_start, carry.method), (run.started, method));
        fx.store_carry(carry);

        // Then C1 passes, its key is listed and the ledger is untouched.
        assert_eq!(fx.failures(), Vec::<String>::new());
        let task = fx.task();
        let audit = completion_check_task(&fx.ctx, &task, SUMMARY).unwrap();
        assert_eq!(audit.evidence_keys, vec!["capture-1".to_string()]);
        let run = task.current_run().unwrap();
        assert_eq!(
            run.completion.evidence[0].source,
            EvidenceSource::WorkerAssertion
        );
        assert_eq!(
            serde_json::to_vec(&run.completion.evidence).unwrap(),
            ledger
        );
        assert_eq!(verification::evidence_digest(run).unwrap(), digest);
    }
}

#[test]
fn carry_never_hides_a_later_defect() {
    type Defect = fn(&Fx);
    // (defect, failure name, relation stays live)
    let cases: [(&str, Defect, &str, bool); 8] = [
        (
            "record bytes edited",
            |fx| {
                fx.mutate(|t| {
                    t.current_run_mut().unwrap().completion.evidence[0]
                        .observation
                        .observed = "edited".into();
                })
            },
            "evidence_stale:C1",
            false,
        ),
        (
            "artifact bytes edited",
            |fx| {
                std::fs::write(fx.ctx.store.dir().join(&fx.artifact), "tampered!!!!!\n").unwrap();
            },
            "evidence_artifact_invalid:C1",
            true,
        ),
        (
            "captured before window_start",
            |fx| {
                fx.mutate(|t| {
                    let carry = t
                        .current_run_mut()
                        .unwrap()
                        .completion
                        .evidence_carry
                        .as_mut();
                    carry.unwrap().window_start = ts("2026-01-03T00:00:00Z");
                })
            },
            "evidence_capture_stale:C1",
            true,
        ),
        (
            "captured after the carry",
            |fx| {
                fx.mutate(|t| {
                    let carry = t
                        .current_run_mut()
                        .unwrap()
                        .completion
                        .evidence_carry
                        .as_mut();
                    carry.unwrap().at = ts("2026-01-01T12:00:00Z");
                })
            },
            "evidence_capture_stale:C1",
            true,
        ),
        (
            "artifact added before window_start",
            |fx| {
                let path = fx.artifact.clone();
                fx.mutate(|t| {
                    let artifact = t.artifacts.iter_mut().find(|a| a.path == path).unwrap();
                    artifact.added = ts("2025-12-31T00:00:00Z");
                })
            },
            "evidence_artifact_invalid:C1",
            true,
        ),
        (
            "task body edited",
            |fx| fx.mutate(|t| t.body = "edited spec".into()),
            "evidence_stale:C1",
            false,
        ),
        (
            "review policy changed",
            |fx| {
                let mut store = fx.ctx.store.reopen().unwrap();
                store.config.review.gate = Some(vec!["changed-gate".into()]);
                store.save_config().unwrap();
            },
            "evidence_stale:C1",
            false,
        ),
        (
            "new gate attempt replaced N",
            |fx| {
                fx.gate("N2");
            },
            "evidence_stale:C1",
            false,
        ),
    ];
    for (defect, apply, name, stays_live) in cases {
        // Given a live carry from A to N.
        let fx = Fx::new(AUDITED);
        fx.carried(CarryMethod::CleanReplay);
        assert_eq!(fx.failures(), Vec::<String>::new(), "{defect}: baseline");

        // When the defect happens.
        apply(&fx);

        // Then today's failure name is reported; an inert relation changes nothing.
        let failures = fx.failures();
        assert!(
            failures.contains(&name.to_string()),
            "{defect}: {failures:?}"
        );
        if stays_live {
            assert_eq!(failures, vec![name.to_string()], "{defect}");
        } else {
            assert_eq!(failures, fx.failures_without_carry(), "{defect}");
        }
    }
}

#[test]
fn establish_refuses_unmet_obligations_before_the_empty_ledger_shortcut() {
    type Setup = fn(&Fx) -> CandidateIdentity;
    let cases: [(&str, Option<Shape>, Setup, &[&str]); 4] = [
        (
            "audited required, empty ledger",
            AUDITED,
            |fx| fx.gate("N1").0,
            &["evidence_missing:C1"],
        ),
        (
            "unaudited, progress complete, no record",
            Some(Shape {
                audited: false,
                required: false,
            }),
            |fx| {
                let spec = verification::spec_digest(&fx.task()).unwrap();
                fx.mutate(|t| {
                t.current_run_mut().unwrap().progress = Some(serde_json::from_value(serde_json::json!({
                    "revision": 1, "spec_revision": spec,
                    "phases": [{"id": "p", "label": "p", "steps": [
                        {"id": "s", "label": "s", "status": {"state": "done"}, "criteria": ["C1"]}
                    ]}]
                })).unwrap());
            });
                fx.gate("N1").0
            },
            &["evidence_missing:C1", "progress_without_evidence:C1"],
        ),
        (
            "latest record failed",
            AUDITED,
            |fx| fx.record(ObservationStatus::Fail),
            &["evidence_failed:C1"],
        ),
        (
            "record bound to another identity",
            AUDITED,
            |fx| {
                fx.record(ObservationStatus::Pass);
                fx.gate("N1").0
            },
            &["evidence_stale:C1"],
        ),
    ];
    for (case, shape, setup, expected) in cases {
        for method in METHODS {
            // Given the source obligation state.
            let fx = Fx::new(shape);
            let source = setup(&fx);
            let started = fx.task().current_run().unwrap().started;
            let (to, _) = fx.gate("M1");
            let task = fx.task();
            let events = fx.ctx.store.events().unwrap();

            // When establishing a carry.
            let failures = refusal(fx.establish(&source, started, &to, method));

            // Then it refuses with today's names and changes nothing.
            assert_eq!(failures, expected.to_vec(), "{case} via {method:?}");
            assert_eq!(fx.task(), task, "{case} via {method:?}");
            assert_eq!(
                fx.ctx.store.events().unwrap(),
                events,
                "{case} via {method:?}"
            );
        }
    }
}

#[test]
fn establish_needs_no_relation_without_obligations_or_records() {
    let cases = [
        ("no manifest", None),
        (
            "only optional criteria",
            Some(Shape {
                audited: true,
                required: false,
            }),
        ),
        (
            "unaudited, nothing complete",
            Some(Shape {
                audited: false,
                required: true,
            }),
        ),
    ];
    for (case, shape) in cases {
        for method in METHODS {
            // Given an empty ledger and no obligation.
            let fx = Fx::new(shape);
            let (source, at) = fx.gate("N1");
            let (to, _) = fx.gate("M1");

            // When establishing a carry, then no relation is needed and the carry proceeds.
            let carry = fx.establish(&source, at, &to, method).unwrap();
            assert_eq!(carry, None, "{case} via {method:?}");
        }
    }
}

#[test]
fn chained_carry_keeps_the_original_origin_and_window() {
    // Given records at A carried to N by a clean replay onto a sibling's landing.
    let fx = Fx::new(AUDITED);
    let (a, n) = fx.carried(CarryMethod::CleanReplay);
    let first = fx
        .task()
        .current_run()
        .unwrap()
        .completion
        .evidence_carry
        .clone()
        .unwrap();
    let started = fx.task().current_run().unwrap().started;

    // When N is replayed again onto a second landing as M and carried from N.
    let n_at = fx.task().current_run().unwrap().gate.as_ref().unwrap().at;
    let (m, _) = fx.gate("M1");
    let second = fx
        .establish(&n, n_at, &m, CarryMethod::CleanReplay)
        .unwrap()
        .unwrap();
    fx.store_carry(second.clone());

    // Then the relation still starts at A's window, only the latest is stored, and it is live.
    assert_eq!((&second.from, second.window_start), (&a, started));
    assert_eq!(
        (&second.to, &second.records_sha256),
        (&m, &first.records_sha256)
    );
    let stored = fx
        .task()
        .current_run()
        .unwrap()
        .completion
        .evidence_carry
        .clone();
    assert_eq!(stored.as_deref(), Some(&second));
    assert_eq!(fx.failures(), Vec::<String>::new());
}

#[test]
fn runs_without_a_relation_report_todays_failures() {
    // Given records at A and a later gate N with no relation stored.
    let fx = Fx::new(AUDITED);
    fx.record(ObservationStatus::Pass);
    fx.gate("N1");

    // Then the failure list is today's full staleness report.
    assert_eq!(
        fx.failures(),
        vec![
            "evidence_stale:C1".to_string(),
            "evidence_capture_stale:C1".into(),
            "evidence_artifact_invalid:C1".into(),
        ]
    );
}

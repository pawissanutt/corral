use super::*;
use corral_core::{Config, EffectivePolicy, EffectiveTaskPolicy, OperatingMode, Store};
use serde_json::json;
use std::collections::BTreeMap;

fn at() -> Timestamp {
    "2026-10-25T23:00:00Z".parse().unwrap()
}

fn fixture(stage: AutoStage, verdict: Option<Verdict>) -> (tempfile::TempDir, Store, Task) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let mut task = store.create("Review", None, Status::Review, "").unwrap();
    task.runs.push(
        serde_json::from_value(json!({
            "id":"R1","agent":"fixture","worktree":"/not-a-repository",
            "branch":"fixture","base":"main","base_sha":"abc",
            "started":"2026-10-25T20:00:00Z","outcome":"done",
            "autopilot":{"stage":stage,"sha":"abc","at":"2026-10-25T21:00:00Z"},
            "review":{"reviewer":"fixture","sha":"abc","verdict":verdict,
                "at":"2026-10-25T22:00:00Z"}
        }))
        .unwrap(),
    );
    (root, store, task)
}

fn policy(task: &Task, config: &Config, refused: &[Action]) -> EffectivePolicy {
    EffectivePolicy {
        mode: OperatingMode::Autopilot,
        paused: false,
        revision: "fixture".into(),
        authorization: Some("grant".into()),
        reasons: Vec::new(),
        tasks: BTreeMap::from([(
            task.id.clone(),
            EffectiveTaskPolicy {
                review: config.task_review_policy(task),
                acceptance_source: corral_core::PolicySource::Task,
                plan_acceptance_source: corral_core::PolicySource::Task,
                refusals: refused
                    .iter()
                    .map(|action| (*action, vec![format!("{action:?} refused")]))
                    .collect(),
            },
        )]),
    }
}

fn assess(task: &Task, config: &Config, policy: &EffectivePolicy) -> Assessment {
    use corral_core::work::{Work, count};
    let n = Neighborhood::new(std::slice::from_ref(task), config, policy);
    let before = Work::ALL.map(count);
    let result = crate::attention::assess(task, &n, at());
    for (index, work) in Work::ALL.iter().enumerate() {
        if *work != Work::BoardVisit {
            assert_eq!(count(*work), before[index], "{work:?}");
        }
    }
    result
}

#[test]
fn completed_verdicts_require_review_and_changes_also_require_repair() {
    for verdict in [Verdict::Approve, Verdict::Changes, Verdict::Escalate] {
        for refused in [
            vec![],
            vec![Action::Review],
            vec![Action::Repair],
            vec![Action::Review, Action::Repair],
            vec![Action::Accept],
            vec![Action::Gate],
        ] {
            // Given a verdict bound to the current run and independently differing grants.
            let (_root, store, task) = fixture(AutoStage::Reviewing, Some(verdict));
            let policy = policy(&task, &store.config, &refused);
            // When the pure assessment consumes the bounded projection.
            let a = assess(&task, &store.config, &policy);
            // Then Review is the entry guard; only Changes needs Repair, not Accept or Gate.
            let blocked = if refused.contains(&Action::Review) {
                Some(Action::Review)
            } else if verdict == Verdict::Changes && refused.contains(&Action::Repair) {
                Some(Action::Repair)
            } else {
                None
            };
            match blocked {
                Some(action) => {
                    assert!(a.progress.is_none());
                    assert_eq!(a.conditions.len(), 1);
                    assert_eq!(a.conditions[0].kind, "authority_blocked");
                    assert_eq!(a.conditions[0].responsible, Responsible::Owner);
                    assert_eq!(a.conditions[0].reason, format!("{action:?} refused"));
                    assert_eq!(
                        a.conditions[0].resume[0].expected,
                        Some(format!("{action:?}"))
                    );
                }
                None => {
                    assert!(a.conditions.is_empty(), "{verdict:?}: {a:?}");
                    assert_eq!(a.progress.unwrap().executor, "autopilot");
                }
            }
        }
    }
}

#[test]
fn changes_stage_uses_repair_regardless_of_gate_permission() {
    for gate_allowed in [false, true] {
        for repair_allowed in [false, true] {
            // Given independently allowed or refused Gate and Repair grants.
            let (_root, store, task) = fixture(AutoStage::Changes, None);
            let mut refused = Vec::new();
            if !gate_allowed {
                refused.push(Action::Gate);
            }
            if !repair_allowed {
                refused.push(Action::Repair);
            }
            let policy = policy(&task, &store.config, &refused);
            // When Changes is assessed.
            let a = assess(&task, &store.config, &policy);
            // Then only Repair determines authority for this stage.
            assert_eq!(a.progress.is_some(), repair_allowed);
            if repair_allowed {
                assert!(a.conditions.is_empty());
            } else {
                assert_eq!(a.conditions.len(), 1);
                assert_eq!(a.conditions[0].kind, "authority_blocked");
                assert_eq!(
                    a.conditions[0].resume[0].expected.as_deref(),
                    Some("Repair")
                );
            }
        }
    }
}

#[test]
fn real_dispatch_pause_and_exhausted_rounds_refuse_only_repair_progression() {
    for paused in [false, true] {
        for stage in [AutoStage::Reviewing, AutoStage::Changes] {
            // Given a real confirmed grant and a production Repair-only refusal.
            let (_root, mut store, mut task) = fixture(stage, Some(Verdict::Changes));
            store
                .edit_config(|doc| doc.set_str("projects.app.path", "."))
                .unwrap();
            store.reload_config_if_changed().unwrap();
            task.project = Some("app".into());
            if !paused {
                task.rounds = store.config.task_review_policy(&task).max_rounds;
            }
            store.save(&task).unwrap();
            let preview = store
                .preview_operating_policy(&corral_core::OperatingSelection {
                    tasks: std::collections::BTreeSet::from([task.id.clone()]),
                    authority: corral_core::OperatingAuthority {
                        dispatch: true,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap();
            store.config = store
                .confirm_operating_policy(&preview, "fixture", at())
                .unwrap();
            if paused {
                store
                    .edit_config(|doc| doc.set_str("dispatch.paused", "true"))
                    .unwrap();
                store.reload_config_if_changed().unwrap();
            }
            let policy = store.effective_operating_policy().unwrap();
            assert!(policy.allows(&task.id, Action::Gate));
            assert!(policy.allows(&task.id, Action::Review));
            assert!(!policy.allows(&task.id, Action::Repair));
            // When the recorded Changes verdict or Changes stage is assessed.
            let a = assess(&task, &store.config, &policy);
            // Then Reviewing can escalate exhausted rounds before Repair, while actual
            // send-back and the Changes stage expose their Repair refusal.
            if !paused && stage == AutoStage::Reviewing {
                assert!(a.conditions.is_empty());
                assert_eq!(a.progress.unwrap().executor, "autopilot");
            } else {
                assert!(a.progress.is_none());
                assert_eq!(a.conditions.len(), 1);
                assert_eq!(a.conditions[0].kind, "authority_blocked");
                assert_eq!(
                    a.conditions[0].resume[0].expected.as_deref(),
                    Some("Repair")
                );
                assert!(a.conditions[0].reason.contains(if paused {
                    "dispatch is paused"
                } else {
                    "automatic repair rounds exhausted"
                }));
            }
        }
    }
}

#[test]
fn reviewing_without_current_verdict_retains_unmatched_stall() {
    for missing_record in [false, true] {
        // Given a historical verdict but no verdict in the current Reviewing run.
        let (_root, store, mut task) = fixture(AutoStage::Reviewing, Some(Verdict::Approve));
        let previous = task.runs[0].clone();
        task.runs[0].id = "R2".into();
        if missing_record {
            task.runs[0].review = None;
        } else {
            task.runs[0].review.as_mut().unwrap().verdict = None;
        }
        task.runs.insert(0, previous);
        let policy = policy(&task, &store.config, &[]);
        // When the task is assessed.
        let a = assess(&task, &store.config, &policy);
        // Then a past run cannot authorize progress or change the canonical stall age.
        assert!(a.progress.is_none());
        assert_eq!(a.conditions.len(), 1);
        assert_eq!(a.conditions[0].kind, "unexpected_stall");
        assert_eq!(a.conditions[0].responsible, Responsible::Pm);
        assert_eq!(
            a.conditions[0].since,
            Some("2026-10-25T21:00:00Z".parse().unwrap())
        );
    }
}

#[test]
fn completed_verdict_preserves_independent_conditions() {
    for refused in [vec![], vec![Action::Review]] {
        // Given an independent silent-turn condition alongside the recorded verdict.
        let (_root, store, mut task) = fixture(AutoStage::Reviewing, Some(Verdict::Approve));
        let mut child = task.runs[0].clone();
        child.id = "rv1".into();
        child.outcome = None;
        child.ended = None;
        child.idle_no_report = true;
        child.review = None;
        task.runs[0].review_runs.push(child);
        let policy = policy(&task, &store.config, &refused);
        // When the verdict is assessed with or without authority.
        let a = assess(&task, &store.config, &policy);
        // Then independent conditions survive both progress and refusal.
        assert!(
            a.conditions
                .iter()
                .any(|condition| condition.kind == "silent_turn")
        );
        assert_eq!(a.progress.is_some(), refused.is_empty());
        assert_eq!(
            a.conditions
                .iter()
                .any(|condition| condition.kind == "authority_blocked"),
            !refused.is_empty()
        );
    }
}

#[test]
fn question_revisions_and_escalations_have_distinct_canonical_conditions() {
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let task = store.create("Question", None, Status::Running, "").unwrap();
    let mut q: AgentQuestion = serde_json::from_value(serde_json::json!({
        "version":1,"key":"T-001/R1/Q1","revision":1,"kind":"clarification","blocking":true,
        "text":"Which constraint?","state":"open",
        "asked":{"request_id":"ask","payload_sha256":"sha","session_id":"session","generation":1,"at":"2026-10-25T20:00:00Z"},
        "wake":{"route":"none","key":"wake","state":"pending"}
    })).unwrap();
    let now = "2026-10-25T23:00:00Z".parse().unwrap();
    let opened = question(&task, &q, now);
    q.state = QuestionState::Escalated;
    q.triage.push(
        serde_json::from_value(serde_json::json!({
            "action":"escalate","by":"pm","revision":1,"at":"2026-10-25T21:00:00Z"
        }))
        .unwrap(),
    );
    let escalated = question(&task, &q, now);
    q.unattended_at = Some("2026-10-25T22:00:00Z".parse().unwrap());
    let unattended = question(&task, &q, now);
    assert_eq!(opened.kind, "question_open");
    assert_eq!(opened.responsible, Responsible::Pm);
    assert_eq!(escalated.kind, "question_escalated");
    assert_eq!(escalated.responsible, Responsible::Owner);
    assert_eq!(
        escalated.since,
        Some("2026-10-25T21:00:00Z".parse().unwrap())
    );
    assert_eq!(unattended.kind, "question_unattended");
    assert_eq!(unattended.since, q.unattended_at);
    assert_ne!(opened.key, escalated.key);
    assert_ne!(escalated.key, unattended.key);
    q.revision = 2;
    assert_ne!(unattended.key, question(&task, &q, now).key);
    assert!(q.answer.is_none());
}

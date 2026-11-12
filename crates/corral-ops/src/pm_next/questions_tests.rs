use super::*;
use crate::pm_workset::PmDecision;
use serde_json::json;

fn question(key: &str, blocking: bool, second: u8) -> PmObservation {
    let mut value = json!({
        "kind": "agent_question", "task": "T-001", "key": key, "revision": 3,
        "question_kind": "clarification", "blocking": blocking, "text": "Choose a route",
        "details": {
            "project": "corral", "blocking": blocking,
            "asked_at": format!("2026-11-11T00:00:{second:02}Z"),
            "needed_by": "2026-11-11T02:00:00Z",
            "options": if blocking { vec![] } else { vec![json!({
                "label": "Use the stable route", "value": "Use the stable route",
                "label_bytes_omitted": 0
            })] },
            "options_omitted": 0,
            "recommendation": if blocking { None } else { Some("Use the stable route") },
            "recommendation_bytes_omitted": 0
        }
    });
    if blocking {
        value["kind"] = json!("owner_question");
        value["reason"] = json!("escalated");
        value.as_object_mut().unwrap().remove("blocking");
    }
    serde_json::from_value(value).unwrap()
}

fn decision(observation: &PmObservation) -> PmDecision {
    let action = card(observation, "corral --root /fixture");
    PmDecision {
        id: format!("decision-{}", observation.task().unwrap()),
        task: observation.task().unwrap().into(),
        run: Some("R1".into()),
        source: "AgentQuestion".into(),
        request_digest: "existing-request-digest".into(),
        action,
        presentation: None,
    }
}

#[test]
fn question_digest_preserves_choices_recommendations_and_question_actions() {
    let observations = [
        question("T-001/R1/Q1", false, 1),
        question("T-001/R1/Q2", true, 3),
        question("T-001/R1/Q3", false, 2),
    ];
    let mut decisions: Vec<_> = observations.iter().map(decision).collect();
    for (index, decision) in decisions.iter_mut().enumerate() {
        decision.id = format!("decision-{index}");
    }
    let before = serde_json::to_value(&decisions).unwrap();
    let mut next = decisions[0].action.clone();
    digest_question_next(
        &decisions,
        &mut next,
        "2026-11-11T00:01:00Z".parse().unwrap(),
    )
    .unwrap();
    let digest = next.question_digest.as_ref().unwrap();
    assert_eq!(digest.questions.len(), 3);
    assert_eq!(digest.questions_omitted, 0);
    assert_eq!(digest.questions[0].key, "T-001/R1/Q2");
    assert_eq!(digest.questions[1].key, "T-001/R1/Q1");
    assert_eq!(digest.questions[2].key, "T-001/R1/Q3");
    assert_eq!(digest.questions[0].age_seconds, 57);
    for item in &digest.questions {
        let original = decisions
            .iter()
            .find(|d| d.action.question_digest.as_ref().unwrap().questions[0].key == item.key)
            .unwrap();
        assert_eq!(item.actions, original.action.options);
        if item.requires_owner {
            assert_eq!(item.actions[0].id, "owner_answer");
            assert_eq!(item.actions[1].id, "owner_deadline");
            assert!(item.actions.iter().all(|action| action.requires_owner));
        } else {
            assert_eq!(item.actions[0].id, "answer");
            assert_eq!(item.actions[1].id, "escalate");
            assert!(item.actions.iter().all(|action| !action.requires_owner));
        }
        assert_eq!(item.decision_id.as_deref(), Some(original.id.as_str()));
        if !item.details.blocking {
            assert_eq!(item.details.options[0].value, "Use the stable route");
            assert_eq!(item.details.options[0].label, "Use the stable route");
            assert_eq!(
                item.details.recommendation.as_deref(),
                Some("Use the stable route")
            );
        }
    }
    assert_eq!(serde_json::to_value(&decisions).unwrap(), before);
    assert!(serde_json::to_vec(&next).unwrap().len() <= DIGEST_CARD_MAX_BYTES);
}

#[test]
fn question_digest_does_not_delay_unrelated_actionable_card() {
    let pending = question("T-001/R1/Q1", true, 1);
    let observations = [
        pending.clone(),
        PmObservation::Dispatch {
            task: "T-002".into(),
        },
    ];
    let mut next = super::super::pm_select(&observations, "corral");
    let before = serde_json::to_value(&next).unwrap();
    digest_question_next(
        &[decision(&pending)],
        &mut next,
        "2026-11-11T00:01:00Z".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(next.kind, PmActionKind::Dispatch);
    assert_eq!(next.task.as_deref(), Some("T-002"));
    assert_eq!(serde_json::to_value(next).unwrap(), before);
}

#[test]
fn question_details_bound_utf8_labels_without_changing_stored_values() {
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let task = store
        .create("Question", None, corral_core::Status::Running, "")
        .unwrap();
    let value = "界".repeat(100);
    let recommendation = "λ".repeat(200);
    let mut source: corral_core::agent::AgentQuestion = serde_json::from_value(json!({
        "version": 1, "key": "T-001/R1/Q1", "revision": 1,
        "asked": {"request_id": "rq", "payload_sha256": "sha", "session_id": "session",
            "generation": 1, "at": "2026-11-11T00:00:00Z"},
        "kind": "clarification", "blocking": false, "text": "Which?",
        "options": vec![value.clone(); 7], "recommendation": recommendation,
        "state": "open", "wake": {"key": "qwake:T-001/R1/Q1:1", "route": "none", "state": "pending"}
    }))
    .unwrap();
    let detail = details(&task, &mut source);
    assert_eq!(detail.options.len(), 6);
    assert_eq!(detail.options_omitted, 1);
    assert_eq!(detail.options[0].label.len(), 120);
    assert_eq!(detail.options[0].label_bytes_omitted, 180);
    assert_eq!(detail.options[0].value, value);
    assert_eq!(detail.recommendation.as_ref().unwrap().len(), 240);
    assert_eq!(detail.recommendation_bytes_omitted, 160);
}

#[test]
fn question_digest_caps_one_batch_at_eight_and_counts_unshown_questions() {
    let decisions: Vec<_> = (0..9)
        .map(|index| {
            let mut decision =
                decision(&question(&format!("T-001/R1/Q{index}"), index == 0, index));
            // Action rendering is independent of the batch-count bound.
            decision.action.options.clear();
            decision
        })
        .collect();
    let mut next = decisions[0].action.clone();
    digest_question_next(
        &decisions,
        &mut next,
        "2026-11-11T00:01:00Z".parse().unwrap(),
    )
    .unwrap();
    let digest = next.question_digest.unwrap();
    assert_eq!(digest.questions.len(), 8);
    assert_eq!(digest.questions_omitted, 1);
    assert_eq!(digest.questions[0].key, "T-001/R1/Q0");
}

#[test]
fn question_digest_text_preserves_each_questions_original_answer_and_escalate_commands() {
    let decisions = [
        decision(&question("T-001/R1/Q1", false, 1)),
        decision(&question("T-001/R1/Q2", false, 2)),
    ];
    let mut next = decisions[0].action.clone();
    digest_question_next(
        &decisions,
        &mut next,
        "2026-11-11T00:01:00Z".parse().unwrap(),
    )
    .unwrap();
    let lines = question_digest_lines(next.question_digest.as_ref().unwrap());
    for decision in &decisions {
        for action in &decision.action.options {
            let prefix = format!("Option {}: {} -> ", action.id, action.label);
            assert_eq!(
                lines
                    .iter()
                    .filter_map(|line| line.strip_prefix(prefix.as_str()))
                    .filter(|command| *command == action.command.as_str())
                    .count(),
                1,
                "{}",
                action.command
            );
        }
    }
}

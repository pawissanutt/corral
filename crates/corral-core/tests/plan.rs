use std::path::Path;

use corral_core::{
    Artifact, ArtifactKind, ConfigDoc, HerdrRef, PlanDocument, PlanRecord, PlanReview,
    PolicySource, Status, Store, Verdict, parse_task, render_task,
};
use serde_yaml_ng::Value;

const YAML: &str = r#"goal: Implement plans
non_goals: []
decisions: []
affected_paths: [crates/corral-core]
contracts: []
dependencies: []
acceptance:
  - scenario: Parse a plan
    expected: Structured data
verification: [cargo test]
owner_decisions: []
"#;

fn frontmatter(yaml: &str) -> String {
    format!("---\n{yaml}---\n# Trailing Markdown\n")
}

#[test]
fn parses_frontmatter_with_optional_criteria_and_trailing_markdown() {
    // Given
    let text = frontmatter(YAML).replace('\n', "\r\n");
    // When
    let document = PlanDocument::parse(&text).unwrap();
    // Then
    assert_eq!(document.affected_paths, ["crates/corral-core"]);
    assert_eq!(document.acceptance[0].criterion, None);
    assert!(document.dependencies.is_empty());
}

#[test]
fn rejects_missing_or_unterminated_frontmatter() {
    // Given
    for text in [YAML.to_string(), format!("---\n{YAML}")] {
        // When
        let result = PlanDocument::parse(&text);
        // Then
        assert!(result.is_err());
    }
}

#[test]
fn requires_every_document_field_even_when_empty_lists_are_allowed() {
    // Given
    let original: Value = serde_yaml_ng::from_str(YAML).unwrap();
    for key in [
        "goal",
        "non_goals",
        "decisions",
        "affected_paths",
        "contracts",
        "dependencies",
        "acceptance",
        "verification",
        "owner_decisions",
    ] {
        let mut yaml = original.clone();
        yaml.as_mapping_mut().unwrap().remove(Value::from(key));
        // When
        let result = PlanDocument::parse(&frontmatter(&serde_yaml_ng::to_string(&yaml).unwrap()));
        // Then
        assert!(result.is_err(), "{key}");
    }
}

#[test]
fn rejects_empty_required_lists_and_blank_string_entries() {
    // Given
    let original: Value = serde_yaml_ng::from_str(YAML).unwrap();
    for (key, value) in [
        ("goal", Value::from(" \t")),
        ("affected_paths", Value::Sequence(vec![])),
        ("acceptance", Value::Sequence(vec![])),
        ("verification", Value::Sequence(vec![])),
        ("non_goals", Value::Sequence(vec![Value::from(" ")])),
        ("decisions", Value::Sequence(vec![Value::from(" ")])),
        ("affected_paths", Value::Sequence(vec![Value::from(" ")])),
        ("contracts", Value::Sequence(vec![Value::from(" ")])),
        ("verification", Value::Sequence(vec![Value::from(" ")])),
        ("owner_decisions", Value::Sequence(vec![Value::from(" ")])),
    ] {
        let mut yaml = original.clone();
        yaml[key] = value;
        // When
        let result = PlanDocument::parse(&frontmatter(&serde_yaml_ng::to_string(&yaml).unwrap()));
        // Then
        assert!(result.is_err(), "{key}");
    }
}

#[test]
fn rejects_unknown_fields_in_document_scopes_and_acceptance() {
    // Given
    for yaml in [
        format!("{YAML}unknown: true\n"),
        YAML.replace("dependencies: []", "dependencies:\n  - task: T-001\n    write_scope: []\n    depends_on: []\n    unknown: true"),
        YAML.replace("    expected: Structured data", "    expected: Structured data\n    unknown: true"),
    ] {
        // When
        let result = PlanDocument::parse(&frontmatter(&yaml));
        // Then
        assert!(result.is_err());
    }
}

#[test]
fn rejects_blank_dependency_and_acceptance_fields() {
    // Given
    for yaml in [
        YAML.replace(
            "dependencies: []",
            "dependencies:\n  - task: ' '\n    write_scope: []\n    depends_on: []",
        ),
        YAML.replace(
            "dependencies: []",
            "dependencies:\n  - task: T-001\n    write_scope: [' ']\n    depends_on: []",
        ),
        YAML.replace(
            "dependencies: []",
            "dependencies:\n  - task: T-001\n    write_scope: []\n    depends_on: [' ']",
        ),
        YAML.replace("scenario: Parse a plan", "scenario: ' '"),
        YAML.replace("expected: Structured data", "expected: ' '"),
        YAML.replace(
            "    expected: Structured data",
            "    expected: Structured data\n    criterion: ' '",
        ),
    ] {
        // When
        let result = PlanDocument::parse(&frontmatter(&yaml));
        // Then
        assert!(result.is_err());
    }
}

#[test]
fn rejects_duplicate_criterion_ids() {
    // Given
    let yaml = YAML.replace(
        "    expected: Structured data",
        "    expected: Structured data\n    criterion: AC-1\n  - scenario: Repeat\n    expected: Rejected\n    criterion: AC-1",
    );
    // When
    let result = PlanDocument::parse(&frontmatter(&yaml));
    // Then
    assert!(result.is_err());
}

#[test]
fn parses_scopes_and_distinct_criteria_without_authorizing_paths() {
    // Given
    let yaml = YAML
        .replace("dependencies: []", "dependencies:\n  - task: T-001\n    write_scope: [/outside]\n    depends_on: [T-002]")
        .replace("    expected: Structured data", "    expected: Structured data\n    criterion: AC-1\n  - scenario: Next\n    expected: Accepted\n    criterion: AC-2");
    // When
    let document = PlanDocument::parse(&frontmatter(&yaml)).unwrap();
    // Then
    assert_eq!(document.dependencies[0].write_scope, ["/outside"]);
    assert_eq!(document.dependencies[0].depends_on, ["T-002"]);
    assert_eq!(document.acceptance[1].criterion.as_deref(), Some("AC-2"));
}

#[test]
fn plan_policy_defaults_to_optional_without_emitting_review_table() {
    // Given
    let config = ConfigDoc::parse("").unwrap().config().unwrap();
    // When
    let required = config.plan_required(None, None);
    // Then
    assert!(!required);
    assert!(!config.review_policy(None, None).plan_required);
    assert_eq!(
        config.review_policy_sources(None, None).plan_required,
        PolicySource::Default
    );
    assert!(!config.to_toml().contains("[review]"));
}

#[test]
fn plan_policy_inherits_global_project_then_task_override() {
    // Given
    let config = ConfigDoc::parse(
        "[review]\nplan_required = true\n[projects.app]\npath = '.'\n[projects.app.review]\nplan_required = false\n",
    ).unwrap().config().unwrap();
    // When
    let resolved = [
        config.plan_required(None, None),
        config.plan_required(Some("missing"), None),
        config.plan_required(Some("app"), None),
        config.plan_required(Some("app"), Some(true)),
        config.plan_required(None, Some(false)),
    ];
    // Then
    assert_eq!(resolved, [true, true, false, true, false]);
    assert_eq!(
        config.review_policy_sources(None, None).plan_required,
        PolicySource::Global
    );
    assert_eq!(
        config
            .review_policy_sources(Some("app"), None)
            .plan_required,
        PolicySource::Project
    );
    let again = ConfigDoc::parse(&config.to_toml())
        .unwrap()
        .config()
        .unwrap();
    assert_eq!(again, config);
}

#[test]
fn old_tasks_remain_optional_and_omit_empty_plan_fields() {
    // Given
    let text = "---\nid: T-001\ntitle: Old task\nstatus: ready\ncreated: 2026-10-15T00:00:00Z\nupdated: 2026-10-15T00:00:00Z\n---\n";
    // When
    let task = parse_task(text, Path::new("T-001.md")).unwrap();
    // Then
    assert_eq!(task.plan_required, None);
    assert!(task.plans.is_empty());
    let yaml: Value = serde_yaml_ng::to_value(&task).unwrap();
    assert!(yaml.get("plan_required").is_none());
    assert!(yaml.get("plans").is_none());
}

#[test]
fn plan_history_and_pending_reviews_round_trip_through_store() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Plan history", None, Status::Ready, "")
        .unwrap();
    assert_eq!(task.plan_required, None);
    assert!(task.plans.is_empty());
    let at = "2026-10-15T00:00:00Z".parse().unwrap();
    let review = PlanReview {
        id: "PR1".into(),
        reviewer: "independent".into(),
        policy_digest: "policy-v1".into(),
        verdict: Some(Verdict::Approve),
        notes: Some("Accepted".into()),
        at,
        herdr: Some(HerdrRef {
            workspace_id: "test".into(),
            pane_id: "pane".into(),
            socket: None,
        }),
        pending_reply: None,
    };
    let record = PlanRecord {
        run: None,
        approval: None,
        id: "P1".into(),
        artifact: Artifact {
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
            run: "R1".into(),
            kind: ArtifactKind::Plan,
            title: "Plan".into(),
            path: "artifacts/plan.md".into(),
            source: "/source/plan.md".into(),
            sha256: "digest".into(),
            bytes: 100,
            added: at,
        },
        spec_revision: "spec-v1".into(),
        document: PlanDocument::parse(&frontmatter(YAML)).unwrap(),
        reviews: vec![
            review.clone(),
            PlanReview {
                id: "PR2".into(),
                verdict: None,
                notes: None,
                herdr: None,
                pending_reply: Some("reply-token".into()),
                ..review
            },
        ],
    };
    task.plan_required = Some(false);
    task.plans = vec![
        record.clone(),
        PlanRecord {
            id: "P2".into(),
            reviews: vec![],
            ..record
        },
    ];
    // When
    store.save(&task).unwrap();
    let restored = store.get(&task.id).unwrap();
    // Then
    assert_eq!(restored.plan_required, Some(false));
    assert_eq!(restored.plans, task.plans);
    assert_eq!(
        parse_task(&render_task(&restored), &restored.path).unwrap(),
        restored
    );
    let yaml = serde_yaml_ng::to_value(&restored).unwrap();
    assert!(yaml["plans"][1].get("reviews").is_none());
    assert!(yaml["plans"][0]["reviews"][1].get("verdict").is_none());
    assert!(yaml["plans"][0]["reviews"][1].get("notes").is_none());
    assert!(yaml["plans"][0]["reviews"][1].get("herdr").is_none());
    assert!(
        yaml["plans"][0]["reviews"][0]
            .get("pending_reply")
            .is_none()
    );
}

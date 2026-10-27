use std::path::{Path, PathBuf};

use corral_core::{
    Config, PolicySource, ReviewOverride, ReviewPolicy, RoleSession, Run, SessionAction,
    SessionPolicy, Status, Store,
};
use serde_json::json;

fn legacy_run() -> Run {
    serde_json::from_value(json!({
        "id": "R1", "agent": "worker", "worktree": "/tmp/worktree",
        "branch": "corral/T-456", "base": "main", "base_sha": "abc",
        "started": "2026-10-26T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn omitted_session_settings_default_to_reuse_without_changing_config_serialization() {
    let config: Config = toml::from_str("").unwrap();
    let layer: ReviewOverride = toml::from_str("").unwrap();
    assert_eq!(layer.worker_session, None);
    assert_eq!(layer.reviewer_session, None);
    assert!(layer.is_empty());
    assert_eq!(SessionPolicy::default(), SessionPolicy::Reuse);
    let policy = config.review_policy(None, None);
    assert_eq!(policy.worker_session, SessionPolicy::Reuse);
    assert_eq!(policy.reviewer_session, SessionPolicy::Reuse);
    assert_eq!(policy, ReviewPolicy::default());
    let sources = config.review_policy_sources(None, None);
    assert_eq!(sources.worker_session, PolicySource::Default);
    assert_eq!(sources.reviewer_session, PolicySource::Default);
    assert!(!config.to_toml().contains("worker_session"));
    assert!(!config.to_toml().contains("reviewer_session"));
    let effective = config.effective_value().unwrap();
    assert_eq!(
        effective["review"]["worker_session"].as_str(),
        Some("reuse")
    );
    assert_eq!(
        effective["review"]["reviewer_session"].as_str(),
        Some("reuse")
    );
}

#[test]
fn fresh_session_policies_are_independent_for_each_role() {
    for role in ["worker_session", "reviewer_session"] {
        let config: Config = toml::from_str(&format!("[review]\n{role} = 'fresh'")).unwrap();
        let policy = config.review_policy(None, None);
        assert_eq!(
            policy.worker_session == SessionPolicy::Fresh,
            role == "worker_session"
        );
        assert_eq!(
            policy.reviewer_session == SessionPolicy::Fresh,
            role == "reviewer_session"
        );
        assert!(!config.review.is_empty());
        assert!(
            !config.has_review_policy(None, None),
            "session policy alone must not activate review"
        );
        let encoded = serde_json::to_value(&policy).unwrap();
        assert_eq!(encoded[role], "fresh");
        let effective = config.effective_value().unwrap();
        assert_eq!(effective["review"][role].as_str(), Some("fresh"));
    }
}

#[test]
fn project_session_settings_inherit_and_override_global_roles_independently() {
    let config: Config = toml::from_str(
        "[review]\nworker_session = 'fresh'\nreviewer_session = 'fresh'\n\
         [projects.inherited]\npath = '.'\n\
         [projects.worker]\npath = '.'\n\
         [projects.worker.review]\nworker_session = 'reuse'\n\
         [projects.reviewer]\npath = '.'\n\
         [projects.reviewer.review]\nreviewer_session = 'reuse'\n",
    )
    .unwrap();
    for (project, worker, reviewer, worker_source, reviewer_source) in [
        (
            "inherited",
            "fresh",
            "fresh",
            PolicySource::Global,
            PolicySource::Global,
        ),
        (
            "worker",
            "reuse",
            "fresh",
            PolicySource::Project,
            PolicySource::Global,
        ),
        (
            "reviewer",
            "fresh",
            "reuse",
            PolicySource::Global,
            PolicySource::Project,
        ),
    ] {
        let policy = config.review_policy(Some(project), None);
        assert_eq!(serde_json::to_value(policy.worker_session).unwrap(), worker);
        assert_eq!(
            serde_json::to_value(policy.reviewer_session).unwrap(),
            reviewer
        );
        let sources = config.review_policy_sources(Some(project), None);
        assert_eq!(sources.worker_session, worker_source);
        assert_eq!(sources.reviewer_session, reviewer_source);
        let effective = config.effective_value().unwrap();
        let actual = effective["projects"][project].get("review");
        let explicit = &config.projects[project].review;
        if explicit.is_empty() {
            assert!(
                actual.is_none(),
                "inherited policy must not materialize a project table"
            );
        } else {
            assert_eq!(actual, Some(&toml::Value::try_from(explicit).unwrap()));
        }
    }
    let decoded: Config = toml::from_str(&config.to_toml()).unwrap();
    assert_eq!(decoded.review, config.review);
    assert_eq!(decoded.projects, config.projects);
}

#[test]
fn session_enums_use_snake_case_and_reject_unknown_values() {
    for (policy, value) in [
        (SessionPolicy::Reuse, "reuse"),
        (SessionPolicy::Fresh, "fresh"),
    ] {
        assert_eq!(serde_json::to_value(policy).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<SessionPolicy>(json!(value)).unwrap(),
            policy
        );
    }
    for (action, value) in [
        (SessionAction::New, "new"),
        (SessionAction::Reused, "reused"),
        (SessionAction::Resumed, "resumed"),
    ] {
        assert_eq!(serde_json::to_value(action).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<SessionAction>(json!(value)).unwrap(),
            action
        );
    }
    assert!(serde_json::from_value::<SessionPolicy>(json!("Fresh")).is_err());
    assert!(serde_json::from_value::<SessionAction>(json!("resume")).is_err());
}

#[test]
fn legacy_run_deserializes_without_a_session_and_omits_it_on_write() {
    let run = legacy_run();
    assert_eq!(run.session, None);
    let value = serde_json::to_value(&run).unwrap();
    assert!(value.get("session").is_none());
    let restored: Run = serde_json::from_value(value).unwrap();
    assert_eq!(restored, run);
}

#[test]
fn role_session_records_roundtrip_with_optional_metadata() {
    for action in [
        SessionAction::New,
        SessionAction::Reused,
        SessionAction::Resumed,
    ] {
        for metadata in [false, true] {
            let record = RoleSession {
                policy: SessionPolicy::Fresh,
                id: metadata.then(|| "session-1".into()),
                transcript: metadata.then(|| PathBuf::from("/tmp/transcript.jsonl")),
                directory: "/tmp/worktree".into(),
                harness: "test".into(),
                model: metadata.then(|| "model".into()),
                vendor: metadata.then(|| "vendor".into()),
                action,
                diagnostic: metadata.then(|| "session selected".into()),
            };
            let json = serde_json::to_value(&record).unwrap();
            let restored: RoleSession = serde_json::from_value(json).unwrap();
            assert_eq!(restored, record);
            let yaml = serde_yaml_ng::to_string(&record).unwrap();
            let restored: RoleSession = serde_yaml_ng::from_str(&yaml).unwrap();
            assert_eq!(restored, record);
        }
    }
}

#[test]
fn worker_and_reviewer_sessions_persist_independently_in_task_records() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("Session persistence", None, Status::Ready, "")
        .unwrap();
    let mut worker = legacy_run();
    worker.session = Some(RoleSession {
        policy: SessionPolicy::Reuse,
        id: Some("worker-session".into()),
        transcript: Some("/tmp/worker.jsonl".into()),
        directory: Path::new("/tmp/worktree").into(),
        harness: "worker-harness".into(),
        model: Some("worker-model".into()),
        vendor: Some("worker-vendor".into()),
        action: SessionAction::Resumed,
        diagnostic: None,
    });
    let mut reviewer = legacy_run();
    reviewer.id = "R1-review1".into();
    reviewer.session = Some(RoleSession {
        policy: SessionPolicy::Fresh,
        id: Some("reviewer-session".into()),
        transcript: None,
        directory: "/tmp/worktree".into(),
        harness: "reviewer-harness".into(),
        model: None,
        vendor: None,
        action: SessionAction::New,
        diagnostic: Some("fresh review".into()),
    });
    worker.review_runs.push(reviewer);
    store
        .update(&task.id, |task| {
            task.runs.push(worker.clone());
            Ok(())
        })
        .unwrap();
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(reopened.get(&task.id).unwrap().runs, vec![worker]);
}

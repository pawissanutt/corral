use super::*;
use corral_core::{EffectiveTaskPolicy, OperatingMode};
use serde_json::json;

fn at() -> Timestamp {
    "2026-10-29T08:00:00Z".parse().unwrap()
}

fn task(id: &str, status: Status) -> Task {
    let mut task = corral_core::parse_task(
        &format!(
            "---\nid: {id}\ntitle: Fixture\nstatus: ready\ncreated: 2026-10-29T08:00:00Z\nupdated: 2026-10-29T08:00:00Z\n---\n"
        ),
        std::path::Path::new("fixture.md"),
    )
    .unwrap();
    task.status = status;
    task.status_since = Some(at());
    task
}

fn subject(aggregate: bool, prior_run: bool) -> Task {
    let mut task = task("T-550", Status::Ready);
    task.tracker = aggregate;
    let dependencies = ["T-551", "T-552", "T-553", "T-554"]
        .map(String::from)
        .to_vec();
    if aggregate {
        task.children = dependencies;
    } else {
        task.depends_on = dependencies;
    }
    if prior_run {
        task.runs.push(
            serde_json::from_value(json!({
                "id":"R1", "agent":"fixture", "worktree":"/fixture", "branch":"fixture",
                "base":"main", "base_sha":"abc", "started":at(), "ended":at(),
                "outcome":"done"
            }))
            .unwrap(),
        );
    }
    task
}

fn assessment(subject: &Task) -> Assessment {
    let config = Config::default_for(std::path::Path::new("/fixture"));
    let policy = EffectivePolicy {
        mode: OperatingMode::Autopilot,
        paused: false,
        revision: "fixture".into(),
        authorization: Some("grant".into()),
        reasons: Vec::new(),
        tasks: BTreeMap::from([(
            subject.id.clone(),
            EffectiveTaskPolicy {
                review: config.task_review_policy(subject),
                acceptance_source: corral_core::PolicySource::Task,
                plan_acceptance_source: corral_core::PolicySource::Task,
                refusals: BTreeMap::new(),
            },
        )]),
    };
    let tasks = vec![
        subject.clone(),
        task("T-551", Status::Running),
        task("T-552", Status::Review),
        task("T-554", Status::Done),
    ];
    let before_tasks = serde_json::to_value(&tasks).unwrap();
    let neighborhood = Neighborhood::new(&tasks, &config, &policy);
    let before_work = corral_core::work::Work::ALL.map(corral_core::work::count);
    let result = assess(subject, &neighborhood, at());
    // Then projection preserves canonical records and performs no external work.
    assert_eq!(serde_json::to_value(&tasks).unwrap(), before_tasks);
    for (index, work) in corral_core::work::Work::ALL.iter().enumerate() {
        assert_eq!(
            corral_core::work::count(*work),
            before_work[index] + u64::from(*work == corral_core::work::Work::AttentionVisit)
        );
    }
    result
}

fn assert_checks(condition: &Condition, ids: &[&str]) {
    assert!(condition.key.is_none());
    assert_eq!(condition.resume.len(), ids.len());
    for (check, id) in condition.resume.iter().zip(ids) {
        assert_eq!(check.kind, "task_status");
        assert_eq!(check.target.as_deref(), Some(*id));
        assert_eq!(check.expected.as_deref(), Some("done"));
        assert_eq!(check.satisfied, Some(false));
        assert!(check.machine_checkable);
    }
}

#[test]
fn aggregate_names_all_unfinished_members_with_current_statuses() {
    // Given an aggregate with running, review, missing, and completed members.
    let subject = subject(true, false);
    // When projected from canonical task statuses.
    let result = assessment(&subject);
    // Then Blocked identifies every unfinished member, including the missing one.
    let blocked = corral_core::attention::blocked(&subject, &result).unwrap();
    assert_eq!(blocked.kind, "aggregate_prerequisite");
    assert_checks(blocked, &["T-551", "T-552", "T-553"]);
    for label in ["T-551 (running)", "T-552 (review)", "T-553 (missing)"] {
        assert!(blocked.reason.contains(label), "{blocked:#?}");
        assert!(blocked.unblocker().contains(label), "{blocked:#?}");
    }
    assert!(!blocked.reason.contains("T-554"));
    assert!(!blocked.unblocker().contains("T-554"));
}

#[test]
fn ready_retry_names_each_unfinished_dependency_with_current_status() {
    // Given a Ready retry with a prior run and mixed dependency statuses.
    let subject = subject(false, true);
    // When projected, then every unfinished dependency retains its own resume check.
    let result = assessment(&subject);
    assert_eq!(
        corral_core::attention::blocked(&subject, &result)
            .unwrap()
            .kind,
        "prerequisite"
    );
    let prerequisites = result
        .conditions
        .iter()
        .filter(|condition| condition.kind == "prerequisite")
        .collect::<Vec<_>>();
    assert_eq!(prerequisites.len(), 3);
    for (condition, (id, status)) in prerequisites.iter().zip([
        ("T-551", "running"),
        ("T-552", "review"),
        ("T-553", "missing"),
    ]) {
        assert_checks(condition, &[id]);
        let label = format!("{id} ({status})");
        assert!(condition.reason.contains(&label), "{condition:#?}");
        assert_eq!(condition.unblocker(), label);
    }
}

#[test]
fn never_started_ready_dependencies_remain_excluded_from_blocked() {
    // Given the same dependencies without any prior execution.
    let subject = subject(false, false);
    // When assessed, then admission checks do not create Blocked membership.
    let result = assessment(&subject);
    assert!(corral_core::attention::blocked(&subject, &result).is_none());
    assert_eq!(
        result
            .conditions
            .iter()
            .filter(|condition| condition.kind == "prerequisite")
            .count(),
        3
    );
}

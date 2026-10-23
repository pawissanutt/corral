use std::collections::BTreeSet;

use corral_core::work::{Work, count};
use corral_core::{OperatingActionKind, OperatingMode, OperatingSelection, Status, Task};
use jiff::{SignedDuration, Timestamp};

use super::Fixture;

fn project_selection(fixture: &Fixture, max_new_tasks: Option<u32>) -> OperatingSelection {
    OperatingSelection {
        projects: BTreeSet::from(["app".into()]),
        include_new: true,
        max_new_tasks,
        authority: fixture.selection().authority,
        ..OperatingSelection::default()
    }
}

fn filed_at(fixture: &Fixture, title: &str, created: Timestamp) -> Task {
    let task = fixture
        .store
        .create(title, Some("app"), Status::Backlog, "Work\n")
        .unwrap();
    fixture
        .store
        .update(&task.id, |task| {
            task.created = created;
            Ok(())
        })
        .unwrap()
}

#[test]
fn project_grant_covers_tasks_filed_after_confirmation_within_its_cap() {
    // Given a project grant that covers new work, capped at one new task.
    let fixture = Fixture::new();
    let confirmed = Timestamp::now();
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.created = confirmed - SignedDuration::from_hours(1);
            Ok(())
        })
        .unwrap();
    let preview = fixture
        .store
        .preview_operating_policy(&project_selection(&fixture, Some(1)))
        .unwrap();
    assert!(preview.policy.scope.tasks.is_empty());
    assert!(preview.policy.scope.projects.contains_key("app"));
    assert_eq!(preview.policy.scope.expires_after_days, Some(7));
    assert!(preview.constraints.contains_key(&fixture.task.id));
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", confirmed)
        .unwrap();
    // When two tasks are filed after confirmation.
    let first = filed_at(&fixture, "First", confirmed + SignedDuration::from_secs(1));
    let second = filed_at(&fixture, "Second", confirmed + SignedDuration::from_secs(2));
    // Then existing work and the first new task are covered, the second exceeds the cap.
    let policy = fixture.store.effective_operating_policy().unwrap();
    assert_eq!(policy.mode, OperatingMode::Autopilot);
    assert!(policy.allows(&fixture.task.id, OperatingActionKind::Dispatch));
    assert!(policy.allows(&first.id, OperatingActionKind::Dispatch));
    assert!(!policy.allows(&second.id, OperatingActionKind::Dispatch));
    // And finishing the first new task does not free its place in the budget.
    fixture
        .store
        .update(&first.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let policy = fixture.store.effective_operating_policy().unwrap();
    assert!(!policy.allows(&second.id, OperatingActionKind::Dispatch));
}

#[test]
fn project_grant_excludes_named_tasks_and_ends_at_its_expiry() {
    // Given a project grant confirmed eight days ago with a seven-day expiry.
    let fixture = Fixture::new();
    let mut selection = project_selection(&fixture, None);
    selection.exclusions = BTreeSet::from([fixture.task.id.clone()]);
    let later = filed_at(&fixture, "Later", Timestamp::now());
    let preview = fixture.store.preview_operating_policy(&selection).unwrap();
    assert!(!preview.constraints.contains_key(&fixture.task.id));
    fixture
        .store
        .confirm_operating_policy(
            &preview,
            "owner",
            Timestamp::now() - SignedDuration::from_hours(8 * 24),
        )
        .unwrap();
    // When the effective policy is resolved now.
    let policy = fixture.store.effective_operating_policy().unwrap();
    // Then the grant no longer authorizes anything and says why.
    assert_eq!(policy.mode, OperatingMode::Assist);
    assert!(!policy.allows(&later.id, OperatingActionKind::Dispatch));
    assert!(!policy.allows(&fixture.task.id, OperatingActionKind::Dispatch));
    assert!(
        policy
            .reasons
            .iter()
            .any(|reason| reason.contains("expired")),
        "{:?}",
        policy.reasons
    );
}

#[test]
fn one_policy_resolution_identifies_a_granted_repository_once() {
    // Given a confirmed project grant covering several tasks.
    let fixture = Fixture::new();
    let preview = fixture
        .store
        .preview_operating_policy(&project_selection(&fixture, None))
        .unwrap();
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", Timestamp::now())
        .unwrap();
    let tasks: Vec<_> = ["First", "Second", "Third"]
        .map(|title| filed_at(&fixture, title, Timestamp::now()))
        .into_iter()
        .collect();
    let start = count(Work::ProjectIdentity);
    // When the effective policy is resolved.
    let policy = fixture.store.effective_operating_policy().unwrap();
    // Then every member is covered from one resolution of the repository.
    for task in &tasks {
        assert!(policy.allows(&task.id, OperatingActionKind::Dispatch));
    }
    assert_eq!(count(Work::ProjectIdentity) - start, 1);
}

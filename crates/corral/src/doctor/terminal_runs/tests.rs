use corral_core::{Run, RunOutcome, Status, Store};

use crate::doctor::Report;

fn run(id: &str) -> Run {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "agent": "generic",
        "worktree": "/nonexistent/corral-doctor-fixture",
        "branch": "corral/fixture",
        "base": "main",
        "base_sha": "abc",
        "started": "2026-10-15T00:00:00Z",
        "ended": "2026-10-15T01:00:00Z",
        "outcome": "done",
        "agent_state": "done"
    }))
    .unwrap()
}

#[test]
fn terminal_runs_warn_for_each_unfinished_field_combination_without_mutating_tasks() {
    for status in [Status::Done, Status::Dropped] {
        for missing_ended in [false, true] {
            for missing_outcome in [false, true] {
                for working in [false, true] {
                    // Given a terminal task, independent of worktree or pane existence.
                    let dir = tempfile::tempdir().unwrap();
                    let store = Store::init(dir.path()).unwrap();
                    let task = store.create("fixture", None, status, "").unwrap();
                    let mut run = run("R1");
                    if missing_ended {
                        run.ended = None;
                    }
                    if missing_outcome {
                        run.outcome = None;
                    }
                    if working {
                        run.agent_state = Some("working".into());
                    }
                    store
                        .update(&task.id, |task| {
                            task.runs.push(run);
                            Ok(())
                        })
                        .unwrap();
                    let before = serde_json::to_value(store.list().unwrap()).unwrap();

                    // When doctor checks the persisted records.
                    let check = super::check(&store).unwrap();

                    // Then any unfinished field warns without blocking startup or repairing data.
                    let unfinished = missing_ended || missing_outcome || working;
                    assert_eq!(check.id, "terminal_runs");
                    assert_eq!(check.ok, !unfinished);
                    assert!(!check.hard);
                    assert_eq!(check.fix.is_some(), unfinished);
                    if unfinished {
                        assert!(check.message.contains(&task.id));
                        assert!(check.message.contains("R1"));
                        assert_eq!(check.message.contains("ended"), missing_ended);
                        assert_eq!(check.message.contains("outcome"), missing_outcome);
                        assert_eq!(check.message.contains("agent_state=working"), working);
                    }
                    let report = Report {
                        checks: vec![check],
                        changes: Vec::new(),
                    };
                    assert!(report.startup_ready());
                    assert_eq!(report.healthy(), !unfinished);
                    assert_eq!(serde_json::to_value(store.list().unwrap()).unwrap(), before);
                }
            }
        }
    }
}

#[test]
fn terminal_runs_ignore_nonterminal_tasks_and_accept_all_finished_outcomes() {
    // Given unfinished nonterminal tasks, completed terminal tasks, and a task with no runs.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
    ] {
        let task = store.create("active", None, status, "").unwrap();
        store
            .update(&task.id, |task| {
                let mut run = run("R1");
                run.ended = None;
                run.outcome = None;
                run.agent_state = Some("working".into());
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
    }
    for status in [Status::Done, Status::Dropped] {
        for outcome in [
            RunOutcome::Done,
            RunOutcome::Abandoned,
            RunOutcome::Superseded,
            RunOutcome::Discarded,
            RunOutcome::Interrupted,
        ] {
            let task = store.create("finished", None, status, "").unwrap();
            store
                .update(&task.id, |task| {
                    let mut run = run("R1");
                    run.outcome = Some(outcome);
                    run.agent_state = None;
                    task.runs.push(run);
                    Ok(())
                })
                .unwrap();
        }
        store.create("no runs", None, status, "").unwrap();
    }

    // When doctor checks all task records.
    let check = super::check(&store).unwrap();

    // Then no terminal run warning or repair advice is needed.
    assert!(check.ok);
    assert!(!check.hard);
    assert!(check.fix.is_none());
}

#[test]
fn terminal_runs_report_old_runs_but_ignore_accounting_only_review_outcomes() {
    // Given an unfinished old run and a completed reviewer accounting window.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store.create("history", None, Status::Done, "").unwrap();
    store
        .update(&task.id, |task| {
            let mut old = run("R1");
            old.ended = None;
            let mut latest = run("R2");
            let mut reviewer = run("R2-review-1");
            // Reviewer accounting windows intentionally finish with ended, not outcome.
            reviewer.outcome = None;
            latest.review_runs.push(reviewer);
            task.runs = vec![old, latest];
            Ok(())
        })
        .unwrap();

    // When doctor checks the complete run history.
    let check = super::check(&store).unwrap();

    // Then the old coding run is identified without a false reviewer warning.
    assert!(!check.ok);
    assert!(check.message.contains(&format!("{} (Done) R1:", task.id)));
    assert!(!check.message.contains("R2"));
}

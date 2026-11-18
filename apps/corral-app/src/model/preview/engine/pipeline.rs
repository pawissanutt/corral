//! Submitting, checking, reviewing and landing: the engine's steps after a worker's `done`.

use corral_protocol::detail::TaskDetail;
use corral_types::{
    Actor, Blocked, CheckKind, CheckResult, EventKind, Next, Op, Origin, Outcome, Role, Status,
};
use serde_json::{Value, json};

use super::super::rollup::{closed, last_worker};
use super::super::sample::{BRANCHES, artifact, finish, gate, review, sha};
use super::super::{ago, event};
use super::{
    Fail, commit, config, dispatch_next, forced, next_done, next_force_approve, next_retry,
    profiles, refuse, set_status, start, touch,
};
use crate::model::daemon::Feed;

pub(super) fn done(feed: &mut Feed, mut rec: TaskDetail, summary: &str) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    if rec.task.status == Status::Review {
        return Ok(());
    }
    if last_worker(&rec).is_none() {
        return Err(refuse(
            "task has not had a worker run",
            vec![dispatch_next(&rec.task)],
        ));
    }
    if !matches!(rec.task.status, Status::Running | Status::Blocked) {
        return Err(refuse(
            "done requires a Running or Blocked task",
            vec![next_retry(&id), next_force_approve(&id)],
        ));
    }
    let Some(worker) = rec
        .runs
        .iter_mut()
        .rev()
        .find(|run| run.role == Role::Worker)
    else {
        return Ok(());
    };
    finish(feed, worker, 0, Outcome::Done);
    if !summary.is_empty() {
        worker.summary = Some(summary.into());
    }
    let (run, sha) = (worker.id.clone(), worker.head_sha.clone());
    let diff = artifact(feed, &rec, "diff", "diff", 0);
    rec.artifacts.push(diff);
    set_status(feed, &mut rec, Status::Review);
    commit(
        feed,
        rec,
        EventKind::Done,
        Some(&run),
        json!({"sha": sha, "summary": summary}),
    );
    Ok(())
}

pub(super) fn run_gate(feed: &mut Feed, mut rec: TaskDetail) -> Result<(), Fail> {
    let Some(run) = last_worker(&rec)
        .filter(|_| !closed(rec.task.status))
        .map(|run| run.id.clone())
    else {
        return Err(refuse(
            "task has no worktree",
            vec![next_retry(&rec.task.id)],
        ));
    };
    let mut check = gate(feed, &rec, true, None, 0);
    let log = artifact(feed, &rec, "log", "gate log", 0);
    check.log = Some(log.reference());
    rec.artifacts.push(log);
    rec.checks.push(check.clone());
    touch(feed, &mut rec);
    let detail = serde_json::to_value(&check).unwrap_or(Value::Null);
    commit(feed, rec, EventKind::Gate, Some(&run), detail);
    Ok(())
}

pub(super) fn start_review(
    feed: &mut Feed,
    mut rec: TaskDetail,
    profile: Option<&str>,
) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    if rec.task.status != Status::Review {
        return Err(refuse(
            "review requires a submitted task",
            vec![next_done(&id), next_force_approve(&id)],
        ));
    }
    let project = config(
        feed,
        &["projects", rec.task.project.as_deref().unwrap_or_default()],
    );
    let Some(profile) = profile.or(project["reviewer"].as_str()).map(str::to_owned) else {
        let next = profiles(feed)
            .into_iter()
            .map(|profile| {
                Next::run(Op::Review {
                    id: id.clone(),
                    profile: Some(profile),
                })
            })
            .collect();
        return Err(refuse("no reviewer profile resolves; set --profile", next));
    };
    // A reviewer session lasts for the task: a live one gets the next round's request.
    let live = rec
        .runs
        .iter()
        .rev()
        .find(|run| run.role == Role::Reviewer && run.ended.is_none())
        .map(|run| run.id.clone());
    let run_id = match live {
        Some(run) => run,
        None => start(feed, &mut rec, Role::Reviewer, &profile),
    };
    touch(feed, &mut rec);
    let detail = json!({"profile": profile, "round": rec.task.round});
    commit(feed, rec, EventKind::ReviewStarted, Some(&run_id), detail);
    Ok(())
}

pub(super) fn verdict(
    feed: &mut Feed,
    mut rec: TaskDetail,
    approve: bool,
    notes: Option<&str>,
) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    if rec.task.status != Status::Review {
        return Err(refuse(
            "verdict requires a task in Review",
            vec![next_done(&id), next_force_approve(&id)],
        ));
    }
    let check = review(feed, &rec, approve, notes, 0);
    rec.checks.push(check);
    let reviewer = rec
        .runs
        .iter()
        .rev()
        .find(|run| run.role == Role::Reviewer)
        .map(|run| run.id.clone());
    if approve {
        for run in rec
            .runs
            .iter_mut()
            .filter(|run| run.role == Role::Reviewer && run.ended.is_none())
        {
            finish(feed, run, 0, Outcome::Done);
        }
    }
    touch(feed, &mut rec);
    let round = rec.task.round;
    let head = last_worker(&rec).and_then(|run| run.head_sha.clone());
    let green = rec.checks.iter().any(|check| {
        check.kind == CheckKind::Gate
            && check.result == CheckResult::Pass
            && Some(&check.key) == head.as_ref()
    });
    let detail = json!({"approve": approve, "notes": notes, "round": round});
    commit(
        feed,
        rec.clone(),
        EventKind::Verdict,
        reviewer.as_deref(),
        detail,
    );
    if approve && green {
        event(
            feed,
            Some(&id),
            None,
            EventKind::Ready,
            Actor::Auto,
            json!({"sha": head}),
        );
    } else if !approve {
        auto_send_back(feed, rec, notes.unwrap_or("changes requested"));
    }
    Ok(())
}

/// A reviewer's `changes`: one more round, or Blocked once the rounds are used.
fn auto_send_back(feed: &mut Feed, mut rec: TaskDetail, notes: &str) {
    let id = rec.task.id.clone();
    let project = config(
        feed,
        &["projects", rec.task.project.as_deref().unwrap_or_default()],
    );
    let max_rounds = project["max_rounds"]
        .as_u64()
        .or_else(|| config(feed, &["defaults", "max_rounds"]).as_u64())
        .unwrap_or(2);
    if u64::from(rec.task.round) >= max_rounds {
        let blocked = Blocked {
            reason: "automatic change rounds exhausted".into(),
            detail: Some(notes.into()),
            next: vec![
                Next::run(Op::Changes {
                    id: id.clone(),
                    text: String::new(),
                }),
                next_retry(&id),
                next_force_approve(&id),
            ],
        };
        set_status(feed, &mut rec, Status::Blocked);
        rec.task.blocked = Some(blocked.clone());
        let detail = serde_json::to_value(&blocked).unwrap_or(Value::Null);
        commit(feed, rec, EventKind::Blocked, None, detail);
        return;
    }
    rec.task.round += 1;
    send_back(feed, rec, notes);
}

/// Work goes back to the worker: its live session, or a new run on the same branch. The notes
/// are automated input, which a paused pi session holds until it resumes.
pub(super) fn send_back(feed: &mut Feed, mut rec: TaskDetail, notes: &str) {
    let live = last_worker(&rec)
        .filter(|run| run.ended.is_none())
        .map(|run| run.id.clone());
    let run = match live {
        Some(run) => run,
        None => {
            let profile =
                last_worker(&rec).map_or_else(|| "omp-sol".to_owned(), |run| run.profile.clone());
            let run = start(feed, &mut rec, Role::Worker, &profile);
            super::super::pi_script::follow(feed, &mut rec, &run, false);
            run
        }
    };
    set_status(feed, &mut rec, Status::Running);
    let session = format!("{}_{run}", rec.task.id);
    let detail = json!({"notes": notes, "round": rec.task.round});
    commit(feed, rec, EventKind::SentBack, Some(&run), detail);
    let notes = format!("[corral] Sent back: {notes}");
    super::super::pi::deliver(feed, &session, &notes, Origin::Auto, None);
}

pub(super) fn approve(
    feed: &mut Feed,
    mut rec: TaskDetail,
    onto: Option<&str>,
    force: bool,
) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    let land = || {
        Next::force(Op::Approve {
            id: id.clone(),
            onto: onto.map(str::to_owned),
        })
    };
    // As the engine does first: a target that is the task's own branch, or that the repository
    // does not have, is refused by name with one step per existing branch, forced as asked.
    if let Some(worker) = last_worker(&rec) {
        let target = onto.unwrap_or(&worker.base);
        let reason = if target == worker.branch {
            Some(format!(
                "cannot land a task branch onto itself ({target:?})"
            ))
        } else if !BRANCHES.contains(&target) {
            Some(format!("landing target branch {target:?} does not exist"))
        } else {
            None
        };
        if let Some(reason) = reason {
            let next = BRANCHES
                .iter()
                .map(|branch| Next {
                    op: Op::Approve {
                        id: id.clone(),
                        onto: Some((*branch).to_owned()),
                    },
                    force,
                })
                .collect();
            return Err(refuse(reason, next));
        }
    }
    let mut skipped = Vec::new();
    if rec.task.status != Status::Review {
        if !force {
            let next = if matches!(rec.task.status, Status::Running | Status::Blocked) {
                next_done(&id)
            } else {
                next_retry(&id)
            };
            return Err(refuse(
                "approve requires a task in Review",
                vec![next, land()],
            ));
        }
        skipped.push("status rule");
    }
    let Some(worker) = last_worker(&rec).cloned() else {
        return Err(refuse(
            "cannot land a missing task branch, even with --force",
            vec![dispatch_next(&rec.task)],
        ));
    };
    let gated = config(
        feed,
        &[
            "projects",
            rec.task.project.as_deref().unwrap_or_default(),
            "gate",
        ],
    )
    .as_array()
    .is_some_and(|gate| !gate.is_empty());
    let green = rec.checks.iter().any(|check| {
        check.kind == CheckKind::Gate
            && check.result == CheckResult::Pass
            && Some(&check.key) == worker.head_sha.as_ref()
    });
    if gated && !green {
        if !force {
            return Err(refuse(
                "approve requires a green gate on the landed commit",
                vec![Next::run(Op::Gate { id: id.clone() }), land()],
            ));
        }
        skipped.push("gate");
    }
    let now = ago(feed, 0);
    let head = worker
        .head_sha
        .clone()
        .unwrap_or_else(|| sha(&format!("{id}/{}", worker.id), 40));
    for run in &mut rec.runs {
        if run.id == worker.id {
            run.outcome = Some(Outcome::Landed);
            run.ended.get_or_insert(now);
            run.head_sha.get_or_insert_with(|| head.clone());
        } else if run.ended.is_none() {
            finish(feed, run, 0, Outcome::Stopped);
        }
    }
    set_status(feed, &mut rec, Status::Done);
    let target = onto.map_or(worker.base.clone(), str::to_owned);
    commit(
        feed,
        rec,
        EventKind::Landed,
        Some(&worker.id),
        json!({"sha": head, "base": target}),
    );
    if force {
        forced(
            feed,
            &id,
            Some(&worker.id),
            json!({"operation": "approve", "skipped": skipped}),
        );
    }
    Ok(())
}

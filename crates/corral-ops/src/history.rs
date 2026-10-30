//! Finished work: when each Done or Dropped task finished and what its approval landed.

use std::collections::HashMap;

use anyhow::Result;
#[cfg(test)]
use corral_core::Event;
use corral_core::backend::{EventQuery, HeadQuery, PageReq};
use corral_core::{Status, Store, Task};
use jiff::Timestamp;
use serde::Serialize;

/// The commit an approval landed and how (`fast_forward`, `rebase` or `keep_branch`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Landed {
    pub sha: String,
    pub strategy: String,
}

impl Landed {
    /// The first 7 characters of the sha.
    pub fn short_sha(&self) -> &str {
        self.sha.get(..7).unwrap_or(&self.sha)
    }

    /// "ff", "rebase" or "kept" (other strategies verbatim).
    pub fn strategy_label(&self) -> &str {
        match self.strategy.as_str() {
            "fast_forward" => "ff",
            "keep_branch" => "kept",
            s => s,
        }
    }
}

/// When a Done or Dropped task finished, and for an approved task what landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finish {
    pub at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landed: Option<Landed>,
}

/// The finish record of every Done and Dropped task in `tasks`, keyed by task id. `at` is the last event that
/// moved the task into its current status ("approved", "discarded", or "moved" to done/dropped); a task with no
/// such event (edited by hand) uses its `updated` time. `landed` comes from the task's last "approved" event.
pub fn finishes(store: &Store, tasks: &[Task]) -> Result<HashMap<String, Finish>> {
    tasks
        .iter()
        .filter(|task| matches!(task.status, Status::Done | Status::Dropped))
        .map(|task| {
            Ok((
                task.id.clone(),
                finish_for(store, &task.id, task.status, task.updated)?,
            ))
        })
        .collect()
}

/// Finish records for precisely the requested terminal heads, on the caller's read view.
pub fn finishes_for(store: &Store, ids: &[String]) -> Result<HashMap<String, Finish>> {
    store
        .heads(&HeadQuery::Ids(ids.to_vec()))?
        .into_iter()
        .filter(|task| matches!(task.status, Status::Done | Status::Dropped))
        .map(|task| {
            Ok((
                task.id.clone(),
                finish_for(store, &task.id, task.status, task.updated)?,
            ))
        })
        .collect()
}

fn finish_for(store: &Store, id: &str, status: Status, updated: Timestamp) -> Result<Finish> {
    let view = store.view()?;
    let query = EventQuery {
        task: Some(id.to_owned()),
        kinds: ["approved", "discarded", "moved"].map(str::to_owned).into(),
        newest_first: true,
        valid_only: true,
        ..EventQuery::default()
    };
    let mut page = PageReq::first(1);
    loop {
        let rows = view.events(&query, page)?;
        for row in rows.items {
            let event = row.event;
            let to = match event.kind.as_str() {
                "approved" => Some(Status::Done),
                "discarded" => Some(Status::Dropped),
                "moved" => event.detail["to"].as_str().and_then(Status::parse),
                _ => None,
            };
            if to != Some(status) {
                continue;
            }
            let landed = if status == Status::Done {
                // A move to Done keeps the latest approval's landing. Query that
                // kind separately rather than walking all the intervening moves.
                latest_landing(store, id)?
            } else {
                None
            };
            return Ok(Finish {
                at: event.ts,
                landed,
            });
        }
        let Some(after) = rows.next else { break };
        page = PageReq {
            after: Some(after),
            limit: 1,
        };
    }
    Ok(Finish {
        at: updated,
        landed: None,
    })
}

fn latest_landing(store: &Store, id: &str) -> Result<Option<Landed>> {
    let view = store.view()?;
    let query = EventQuery {
        task: Some(id.to_owned()),
        kinds: vec!["approved".into()],
        newest_first: true,
        valid_only: true,
        ..EventQuery::default()
    };
    let mut page = PageReq::first(1);
    loop {
        let rows = view.events(&query, page)?;
        for row in rows.items {
            if let Some(sha) = row.event.detail["sha"].as_str() {
                return Ok(Some(Landed {
                    sha: sha.to_owned(),
                    strategy: row.event.detail["strategy"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                }));
            }
        }
        match rows.next {
            Some(next) => {
                page = PageReq {
                    after: Some(next),
                    limit: 1,
                }
            }
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
fn finishes_from(events: &[Event], tasks: &[Task]) -> HashMap<String, Finish> {
    let finished: HashMap<&str, &Task> = tasks
        .iter()
        .filter(|t| matches!(t.status, Status::Done | Status::Dropped))
        .map(|t| (t.id.as_str(), t))
        .collect();
    let mut out: HashMap<String, Finish> = HashMap::new();
    for ev in events {
        let Some(task) = ev.task.as_deref().and_then(|id| finished.get(id)) else {
            continue;
        };
        let to = match ev.kind.as_str() {
            "approved" => Some(Status::Done),
            "discarded" => Some(Status::Dropped),
            "moved" => ev.detail["to"].as_str().and_then(Status::parse),
            _ => None,
        };
        if to != Some(task.status) {
            continue;
        }
        let entry = out.entry(task.id.clone()).or_insert(Finish {
            at: ev.ts,
            landed: None,
        });
        entry.at = ev.ts;
        if ev.kind == "approved"
            && let Some(sha) = ev.detail["sha"].as_str()
        {
            entry.landed = Some(Landed {
                sha: sha.to_string(),
                strategy: ev.detail["strategy"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    for (id, t) in finished {
        out.entry(id.to_string()).or_insert(Finish {
            at: t.updated,
            landed: None,
        });
    }
    out
}

/// Every Done and Dropped task with its finish record, newest first (ties: higher id first).
pub fn finished_tasks(store: &Store) -> Result<Vec<(Task, Finish)>> {
    let tasks = store.list()?;
    let mut finishes = finishes(store, &tasks)?;
    let mut out: Vec<(Task, Finish)> = tasks
        .into_iter()
        .rev()
        .filter_map(|t| finishes.remove(&t.id).map(|f| (t, f)))
        .collect();
    out.sort_by_key(|row| std::cmp::Reverse(row.1.at));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: &str, status: Status, updated: i64) -> Task {
        let ts = Timestamp::from_second(updated).unwrap();
        Task {
            park: None,
            landed: None,
            infra_wait: None,
            attention: Default::default(),
            dashboard: None,
            origin: None,
            idea_decision: None,
            comments: Vec::new(),
            completion: None,
            result: None,
            plan_required: None,
            design: false,
            plan: None,
            tracker: false,
            approved_scope: None,
            approved_scope_revision: 0,
            approved_scope_history: Vec::new(),
            membership_revision: 0,
            aggregate_attempts: Vec::new(),
            parent: None,
            source_digest: None,
            children: Vec::new(),
            plan_runs: Vec::new(),
            plans: Vec::new(),
            id: id.into(),
            title: id.into(),
            status,
            status_since: None,
            project: None,
            kind: None,
            size: None,
            difficulty: None,
            priority: "P2".into(),
            depends_on: Vec::new(),
            labels: Vec::new(),
            write_scope: Vec::new(),
            profile: None,
            reviewer: None,
            queued: false,
            dispatch_plan: None,
            launch: None,
            dispatch_hold: false,
            review_reservation: None,
            review_wait: None,
            review: None,
            rounds: 0,
            created: ts,
            updated: ts,
            runs: Vec::new(),
            artifacts: Vec::new(),
            agent_origin: None,
            agent_pm_receipts: Vec::new(),
            body: String::new(),
            path: Default::default(),
            member_gates: Default::default(),
        }
    }

    fn ev(id: &str, kind: &str, at: i64, detail: serde_json::Value) -> Event {
        Event {
            seq: 0,
            ts: Timestamp::from_second(at).unwrap(),
            task: Some(id.into()),
            kind: kind.into(),
            actor: None,
            via: None,
            detail,
        }
    }

    #[test]
    fn derived_finish_page_equals_baseline_including_no_runs_and_retained_approval() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let mut tasks = Vec::new();
        let mut seed = 17_u64;
        for i in 0..18 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let status =
                [Status::Done, Status::Dropped, Status::Review][usize::try_from(seed % 3).unwrap()];
            let task = store
                .create(&format!("Task {i}"), None, status, "")
                .unwrap();
            if i % 4 != 0 {
                store
                    .log_event(
                        Some(&task.id),
                        "approved",
                        json!({"sha":"abcdef","strategy":"rebase"}),
                    )
                    .unwrap();
                store
                    .log_event(Some(&task.id), "moved", json!({"to":"done"}))
                    .unwrap();
                store
                    .log_event(Some(&task.id), "approved", json!({"sha":null}))
                    .unwrap();
                store
                    .log_event(Some(&task.id), "discarded", json!({}))
                    .unwrap();
            }
            tasks.push(task);
        }
        let events = store.events().unwrap();
        let expected = finishes_from(&events, &tasks);
        assert_eq!(finishes(&store, &tasks).unwrap(), expected);
        let ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
        assert_eq!(finishes_for(&store, &ids).unwrap(), expected);
        for task in &tasks {
            assert_eq!(
                finishes_for(&store, std::slice::from_ref(&task.id)).unwrap(),
                expected
                    .iter()
                    .filter(|(id, _)| *id == &task.id)
                    .map(|(id, finish)| (id.clone(), finish.clone()))
                    .collect()
            );
        }
    }

    #[test]
    fn derived_finish_page_work_ignores_other_tasks_and_earlier_history() {
        use corral_core::work::{self, Counter, HotScope};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Done without a run", None, Status::Done, "")
            .unwrap();
        store
            .log_event(
                Some(&task.id),
                "approved",
                json!({"sha":"abc","strategy":"rebase"}),
            )
            .unwrap();
        let measure = || {
            let _scope = HotScope::enter();
            let before = work::snapshot();
            let result = finishes_for(&store, std::slice::from_ref(&task.id)).unwrap();
            let counts = work::snapshot().since(&before);
            counts.check_identity().unwrap();
            assert_eq!(counts.unattributed_hydrations(), 0);
            (result, counts)
        };
        let (before, base) = measure();
        for i in 0..10 {
            let old = store.create("Other", None, Status::Done, "").unwrap();
            store
                .log_event(Some(&old.id), "approved", json!({"sha":"other","round":i}))
                .unwrap();
            store
                .log_event(Some(&task.id), "unrelated", json!({"i":i}))
                .unwrap();
        }
        let (after, grown) = measure();
        assert_eq!(before, after);
        for counter in Counter::ALL {
            assert_eq!(base.get(counter), grown.get(counter), "{counter:?}");
        }
    }

    #[test]
    fn finish_comes_from_the_event_that_reached_the_current_status() {
        let tasks = [
            task("T-001", Status::Done, 900),
            task("T-002", Status::Dropped, 900),
            task("T-003", Status::Done, 900),
            task("T-004", Status::Review, 900),
        ];
        let events = [
            ev(
                "T-001",
                "approved",
                100,
                json!({"sha": "1a2b3c4d5e", "strategy": "rebase"}),
            ),
            ev("T-001", "artifact", 200, json!({})),
            ev(
                "T-002",
                "moved",
                150,
                json!({"from": "backlog", "to": "done"}),
            ),
            ev("T-002", "discarded", 300, json!({"run": "R1"})),
            ev("T-004", "approved", 50, json!({"sha": "ffff"})),
        ];
        let f = finishes_from(&events, &tasks);
        let t1 = &f["T-001"];
        assert_eq!(
            t1.at.as_second(),
            100,
            "later non-finishing events do not move it"
        );
        let landed = t1.landed.as_ref().unwrap();
        assert_eq!(
            (landed.short_sha(), landed.strategy_label()),
            ("1a2b3c4", "rebase")
        );
        assert_eq!(
            f["T-002"].at.as_second(),
            300,
            "the move to done is not a drop"
        );
        assert!(f["T-002"].landed.is_none());
        assert_eq!(f["T-003"].at.as_second(), 900, "no event: updated");
        assert!(!f.contains_key("T-004"), "only Done and Dropped tasks");
    }
}

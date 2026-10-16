//! Finished work: when each Done or Dropped task finished and what its approval landed.

use std::collections::HashMap;

use anyhow::Result;
use corral_core::{Event, Status, Store, Task};
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
    Ok(finishes_from(&store.events()?, tasks))
}

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
    out.sort_by(|a, b| b.1.at.cmp(&a.1.at));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: &str, status: Status, updated: i64) -> Task {
        let ts = Timestamp::from_second(updated).unwrap();
        Task {
            origin: None,
            idea_decision: None,
            comments: Vec::new(),
            completion: None,
            plan_required: None,
            design: false,
            plan: None,
            children: Vec::new(),
            plan_runs: Vec::new(),
            plans: Vec::new(),
            id: id.into(),
            title: id.into(),
            status,
            project: None,
            kind: None,
            size: None,
            priority: "P2".into(),
            depends_on: Vec::new(),
            labels: Vec::new(),
            write_scope: Vec::new(),
            profile: None,
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
            body: String::new(),
            path: Default::default(),
        }
    }

    fn ev(id: &str, kind: &str, at: i64, detail: serde_json::Value) -> Event {
        Event {
            ts: Timestamp::from_second(at).unwrap(),
            task: Some(id.into()),
            kind: kind.into(),
            detail,
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

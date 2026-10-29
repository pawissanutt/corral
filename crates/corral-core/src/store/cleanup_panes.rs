//! The cleanup ledger (plan T-253 section 4, "Pane references and cleanup"): one row per pane
//! a task's records named that cleanup closes, written in the operation that records the pane
//! and never deleted when its record is replaced, so a replaced gate or reviewer pane stays
//! owed. A row is open until cleanup confirms its pane closed or absent; cleanup reads only
//! the open rows of its scope.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::codec::{Rows, SELF, seq};
use super::head::TaskHead;
use super::head_mut::TaskHeadMut;
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::work::Counter;

/// One unreleased pane of the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupPane {
    /// The row key: `<owner>/<kind>/<pane>`.
    #[serde(skip)]
    pub key: String,
    /// `run` (a coding run's own pane), `gate`, `review`, `plan_review`, `workspace` (a
    /// tool pane opened in its own fallback workspace) or `workspace_debt` (a released pane's
    /// workspace still owed; its `pane` is empty).
    pub kind: String,
    pub pane: String,
    /// The recorded server socket, `""` when the record has none.
    pub socket: String,
    /// The recorded workspace, for run, plan-review and fallback workspace panes.
    pub workspace: Option<String>,
}

fn ledger_row(owner: &str, pane: CleanupPane) -> HistoryRow {
    let mut index = vec![format!("pane:{}", pane.pane)];
    index.extend(pane.workspace.iter().map(|w| format!("workspace:{w}")));
    HistoryRow {
        part: TaskPart::CleanupPane,
        key: format!("{owner}/{}/{}", pane.kind, pane.pane),
        current: false,
        open: true,
        index,
        doc: serde_json::to_value(&pane).unwrap_or(Value::Null),
    }
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn tool_panes(owner: &str, doc: &Value, out: &mut Vec<HistoryRow>) {
    if let Some(pane) = text(&doc["gate"]["pane"]) {
        out.push(ledger_row(
            owner,
            CleanupPane {
                key: String::new(),
                kind: "gate".into(),
                pane,
                socket: text(&doc["gate"]["socket"]).unwrap_or_default(),
                workspace: None,
            },
        ));
    }
    let review = &doc["review"];
    if review["reviewer"].as_str().is_some_and(|r| r != "pm")
        && let Some(pane) = text(&review["pane"])
    {
        out.push(ledger_row(
            owner,
            CleanupPane {
                key: String::new(),
                kind: "review".into(),
                pane,
                socket: String::new(),
                workspace: None,
            },
        ));
    }
}

fn herdr_pane(owner: &str, kind: &str, herdr: &Value, out: &mut Vec<HistoryRow>) {
    if let Some(pane) = text(&herdr["pane_id"]) {
        out.push(ledger_row(
            owner,
            CleanupPane {
                key: String::new(),
                kind: kind.into(),
                pane,
                socket: text(&herdr["socket"]).unwrap_or_default(),
                workspace: text(&herdr["workspace_id"]),
            },
        ));
    }
}

/// The ledger rows a stored container row names: a coding run's own pane, gate and reviewer
/// panes; a review child's gate and reviewer panes; a non-PM plan reviewer's pane.
pub(crate) fn ledger_of(row: &HistoryRow) -> Vec<HistoryRow> {
    let mut out = Vec::new();
    match row.part {
        TaskPart::Run => {
            let owner = format!("run:{}/{}", row.key, seq(SELF));
            herdr_pane(&owner, "run", &row.doc["herdr"], &mut out);
            tool_panes(&owner, &row.doc, &mut out);
        }
        TaskPart::ReviewRun => tool_panes(&format!("run:{}", row.key), &row.doc, &mut out),
        TaskPart::PlanReview if row.doc["reviewer"].as_str().is_some_and(|r| r != "pm") => {
            herdr_pane(
                &format!("plan_review:{}", row.key),
                "plan_review",
                &row.doc["herdr"],
                &mut out,
            );
        }
        _ => {}
    }
    out
}

/// Adds the ledger rows of every container row in `rows` (a full split).
pub(crate) fn derive_ledger(rows: &mut Rows) {
    let derived: Vec<HistoryRow> = rows.0.values().flat_map(ledger_of).collect();
    for row in derived {
        rows.put(row);
    }
}

/// The ledger rows of the tool panes and fallback workspaces only a legacy task's creation events
/// name (`gate_started`, a non-PM `review_started`, `run_workspace_created`), as the legacy
/// cleanup collected them; `events` are that task's events. Rows `rows` already holds stay.
pub(crate) fn derive_event_ledger(rows: &mut Rows, runs: &[crate::Run], events: &[crate::Event]) {
    for event in events {
        let Some(run) = event.detail["run"].as_str() else {
            continue;
        };
        let Some(n) = runs.iter().position(|r| r.id == run) else {
            continue;
        };
        let kind = match event.kind.as_str() {
            "gate_started" => "gate",
            "review_started" if event.detail["reviewer"].as_str().is_some_and(|r| r != "pm") => {
                "review"
            }
            "run_workspace_created" => "workspace",
            _ => continue,
        };
        let Some(pane) = text(&event.detail["pane"]) else {
            continue;
        };
        let row = ledger_row(
            &format!("run:{}/{}", seq(n + 1), seq(SELF)),
            CleanupPane {
                key: String::new(),
                kind: kind.into(),
                pane,
                socket: text(&event.detail["socket"]).unwrap_or_default(),
                workspace: (kind == "workspace")
                    .then(|| text(&event.detail["workspace"]))
                    .flatten(),
            },
        );
        if !rows.0.contains_key(&(row.part, row.key.clone())) {
            rows.put(row);
        }
    }
}

/// A full write keeps the ledger as stored: rows it still derives keep their release state,
/// and rows whose record was replaced stay owed.
pub(crate) fn carry_ledger(
    old: &std::collections::BTreeMap<(TaskPart, String), HistoryRow>,
    new: &mut std::collections::BTreeMap<(TaskPart, String), HistoryRow>,
) {
    for (key, row) in old {
        if key.0 == TaskPart::CleanupPane {
            new.insert(key.clone(), row.clone());
        }
    }
}

/// A workspace a released pane left owed: kind `workspace_debt`, no pane, keyed outside every
/// run's subtree so a Round cleanup never reads it.
fn workspace_debt(workspace: &str, socket: &str) -> HistoryRow {
    let debt = CleanupPane {
        key: String::new(),
        kind: "workspace_debt".into(),
        pane: String::new(),
        socket: socket.into(),
        workspace: Some(workspace.into()),
    };
    HistoryRow {
        part: TaskPart::CleanupPane,
        key: format!("debt/workspace/{workspace}"),
        current: false,
        open: true,
        index: vec![format!("workspace:{workspace}")],
        doc: serde_json::to_value(&debt).unwrap_or(Value::Null),
    }
}

fn decode(rows: Vec<HistoryRow>) -> Result<Vec<CleanupPane>> {
    rows.into_iter()
        .map(|row| {
            let mut pane: CleanupPane = serde_json::from_value(row.doc)?;
            pane.key = row.key;
            Ok(pane)
        })
        .collect()
}

impl TaskHead {
    /// The open ledger rows of the current coding run's subtree (`round`), or of the whole
    /// task: only panes cleanup has not confirmed closed or absent.
    ///
    /// # Errors
    /// Returns read or decode errors.
    pub fn unreleased_panes(&self, round: bool) -> Result<Vec<CleanupPane>> {
        let q = if round {
            let Some(s) = self.seqs.run else {
                return Ok(Vec::new());
            };
            RowQuery::under(TaskPart::CleanupPane, format!("run:{}/", seq(s))).open()
        } else {
            RowQuery::part(TaskPart::CleanupPane).open()
        };
        decode(self.query(&q, u32::MAX, Counter::NestedRowDecodes)?)
    }

    /// The ledger rows naming `pane`, released or not: the sweep's retry net for a listed
    /// pane a terminal task recorded.
    ///
    /// # Errors
    /// Returns read or decode errors.
    pub fn ledger_named(&self, pane: &str) -> Result<Vec<CleanupPane>> {
        let q = RowQuery::part(TaskPart::CleanupPane).with_index(format!("pane:{pane}"));
        decode(self.query(&q, u32::MAX, Counter::NestedRowDecodes)?)
    }
}

impl TaskHeadMut {
    /// Records the fallback workspace a tool pane of the current run `run` was opened in, in
    /// the operation that logs its creation.
    ///
    /// # Errors
    /// Refuses a run that is not current.
    pub fn record_run_workspace(
        &mut self,
        run: &str,
        pane: &str,
        workspace: &str,
        socket: &str,
    ) -> Result<()> {
        let s = self.current_run_seq(run)?;
        self.put(ledger_row(
            &format!("run:{}/{}", seq(s), seq(SELF)),
            CleanupPane {
                key: String::new(),
                kind: "workspace".into(),
                pane: pane.into(),
                socket: socket.into(),
                workspace: Some(workspace.into()),
            },
        ));
        Ok(())
    }

    /// Releases the ledger rows `keys`: cleanup confirmed their panes closed or absent. Each
    /// `(workspace, socket)` of `owed` is a released pane's workspace not yet seen gone; it
    /// stays owed as one workspace row, outside every run's subtree, that only a Task cleanup
    /// reads and settles.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn release_panes(&mut self, keys: &[String], owed: &[(String, String)]) -> Result<()> {
        for (workspace, socket) in owed {
            self.put(workspace_debt(workspace, socket));
        }
        for key in keys {
            let q = RowQuery::key(TaskPart::CleanupPane, key.clone());
            if let Some(mut row) = self
                .rows_now(&q, Counter::NestedRowDecodes)?
                .into_iter()
                .next()
                .filter(|row| row.open)
            {
                row.open = false;
                self.put(row);
            }
        }
        Ok(())
    }
}

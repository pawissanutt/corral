//! A Code Mode cell, as `code-cell.ts` draws it: one header row in the tool grammar (`code 7
//! calls ✓5 ⠋2 · read ×3, bash  cell 12  14s`) and under it the calls the cell actually made,
//! from the runtime's records of the cell (`details.cells`, or the settlement the host recorded
//! once a detached cell finished), never from parsing the script. Each call is drawn by the same
//! presenter as a direct call of that tool. Then what the script printed or returned, and the
//! error; opened, every call and the script under `source`.

use serde_json::Value;

use crate::model::hue::Hue;

use super::replay::{Cx, Rest};
use super::tool::{
    Call, CallState, ToolState, count_of, elapsed, list_of, parse, plural, present, str_of,
};
use super::{Action, More, Tool};

/// Above this many calls a folded cell keeps the first three, the last ok one, at most three
/// running and at most eight failed calls; the rest fold into one row per state.
const FOLD_ABOVE: usize = 5;
const FOLD_HEAD: usize = 3;
const RUNNING_SHOWN: usize = 3;
const FAILED_SHOWN: usize = 8;
/// Rows of printed output and return value while folded.
const OUTPUT_ROWS: usize = 5;
/// Script lines shown while it streams in.
const WRITING_TAIL: usize = 5;
/// A cell report status that says the cell is over.
const FINAL: [&str; 7] = [
    "ok",
    "error",
    "cancelled",
    "timeout",
    "reset",
    "closed",
    "not_found",
];

/// A Code Mode call and the calls its cell made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeCell {
    pub state: ToolState,
    /// `7 calls`, `no tool calls`; a `cancel` or `reset` call's one row
    /// (`cancel cell 2 · stopped after 3.2s, 4 calls`).
    pub calls: String,
    /// How many calls ended each way: `✓5`, spinner `2`, `!1`, `✗1`.
    pub tally: Vec<(ToolState, u64)>,
    /// The tools it used, in first-call order: `read ×3, bash`.
    pub tools: String,
    /// How the cell ended when not ok (`failed`, `timed out`), in the failure colour.
    pub ended: Option<String>,
    /// `cell 12`, or `in background` while a detached cell runs.
    pub label: Option<String>,
    pub time: Option<String>,
    /// A detached cell's sentence: `the agent continues; this row updates when the cell
    /// settles`, then `settled in background`.
    pub note: Option<String>,
    /// The calls it made, in call order, with folded runs.
    pub children: Vec<CellRow>,
    /// `error`, `printed`, `returned`.
    pub output: Vec<CellOutput>,
    /// The newest lines of the script while it is written.
    pub writing: Vec<String>,
    /// The script, while the cell is opened.
    pub source: Option<String>,
    pub open: bool,
    pub more: More,
    pub action: Option<Action>,
}

/// A row under the cell's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellRow {
    Call(Tool),
    /// `… 12 more ✓`, `… 4 more running`, `… 2 more ✗`.
    Fold {
        state: ToolState,
        words: String,
    },
}

/// A labelled piece of what the cell produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellOutput {
    pub label: &'static str,
    pub text: String,
    pub hue: Hue,
}

/// `cell-12` as `cell 12`.
fn cell_label(cell: &str) -> String {
    match cell.strip_prefix("cell-") {
        Some(number) => format!("cell {number}"),
        None => cell.to_owned(),
    }
}

/// A nested call's time: `40ms`, then as a direct call's.
fn nested_time(ms: u64) -> Option<String> {
    if ms < 1000 {
        Some(format!("{ms}ms"))
    } else {
        elapsed(ms)
    }
}

/// One call the cell made, drawn by its tool's presenter.
fn child(outcome: &Value, settled: bool, cx: Cx<'_>) -> Tool {
    let name = str_of(outcome, "tool").unwrap_or("tool");
    let ok = outcome.get("ok").and_then(Value::as_bool);
    let text = str_of(outcome, "preview")
        .or_else(|| str_of(outcome, "message"))
        .unwrap_or_default();
    let call = Call {
        name,
        args: str_of(outcome, "args").map_or(Value::Null, parse),
        details: outcome.get("structured").cloned().unwrap_or(Value::Null),
        text,
        state: match ok {
            Some(true) => CallState::Done,
            Some(false) => CallState::Failed,
            None => CallState::Running,
        },
        session: cx.id,
        journal: cx.journal,
        lines: count_of(outcome, "resultLines"),
    };
    let mut row = present(&call);
    if ok.is_none() && settled {
        row.state = ToolState::Unknown;
        row.outcome = "outcome not recorded".to_owned();
        row.hue = Hue::Held;
    }
    let conversation = str_of(outcome, "conversationId").and_then(|id| {
        let view = super::tool::conversation_view(name, cx.id, id)?;
        row.view = Some(view);
        Some(format!("Conversation ID: {id}"))
    });
    let late = outcome.get("late").and_then(Value::as_bool) == Some(true);
    Tool {
        state: row.state,
        name: row.verb,
        summary: row.target,
        outcome: row.outcome,
        outcome_hue: row.hue,
        detail: row.detail,
        note: if late {
            Some("after the cell".to_owned())
        } else {
            row.right
        },
        hidden: None,
        time: count_of(outcome, "durationMs").and_then(nested_time),
        view: row.view,
        // A failed call shows its first reason row.
        tail: row
            .body
            .into_iter()
            .take(usize::from(ok == Some(false)))
            .chain(conversation)
            .map(|row| super::line(&row, super::LineClass::Dim))
            .collect(),
        open: false,
        body: Vec::new(),
        more: More::Whole,
        action: None,
        fold: super::group::Fold {
            key: String::new(),
            tool: name.to_owned(),
            facet: row.facet,
            ms: count_of(outcome, "durationMs").unwrap_or(0),
        },
    }
}

/// The calls of a folded cell: every one up to five, else the first three, the last ok one, at
/// most three running and eight failed ones, the rest of each state folded where the first of
/// them stood.
fn folded(rows: Vec<Tool>, open: bool) -> Vec<CellRow> {
    if open || rows.len() <= FOLD_ABOVE {
        return rows.into_iter().map(CellRow::Call).collect();
    }
    let last_ok = rows.iter().rposition(|row| row.state == ToolState::Ok);
    let mut running = 0;
    let mut failed = 0;
    let mut kept: Vec<bool> = Vec::with_capacity(rows.len());
    for (at, row) in rows.iter().enumerate() {
        let keep = at < FOLD_HEAD
            || Some(at) == last_ok
            || match row.state {
                ToolState::Running | ToolState::Writing => {
                    running += 1;
                    running <= RUNNING_SHOWN
                }
                ToolState::Failed => {
                    failed += 1;
                    failed <= FAILED_SHOWN
                }
                _ => false,
            };
        kept.push(keep);
    }
    let mut out: Vec<CellRow> = Vec::new();
    let mut slots: Vec<(ToolState, usize, u64)> = Vec::new();
    for (row, keep) in rows.into_iter().zip(kept) {
        if keep {
            out.push(CellRow::Call(row));
            continue;
        }
        let state = match row.state {
            ToolState::Writing => ToolState::Running,
            other => other,
        };
        match slots.iter_mut().find(|(known, _, _)| *known == state) {
            Some((_, _, count)) => *count += 1,
            None => {
                slots.push((state, out.len(), 1));
                out.push(CellRow::Fold {
                    state,
                    words: String::new(),
                });
            }
        }
    }
    for (state, at, count) in slots {
        let words = match state {
            ToolState::Running => format!("… {count} more running"),
            ToolState::Ok => format!("… {count} more ✓"),
            ToolState::Failed => format!("… {count} more ✗"),
            ToolState::Unknown => format!("… {count} more !"),
            _ => format!("… {count} more"),
        };
        out[at] = CellRow::Fold { state, words };
    }
    out
}

/// Text output, JSON compacted to one row; folded, at most five rows in all.
fn outputs(report: &Value, open: bool) -> Vec<CellOutput> {
    let compact = |text: &str| -> String {
        let trimmed = text.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
                return value.to_string();
            }
        }
        text.trim_end().to_owned()
    };
    let mut parts = Vec::new();
    if let Some(error) = report.get("error") {
        let message = str_of(error, "message").unwrap_or_default();
        let words = match str_of(error, "name") {
            Some(name) => format!("{name}: {message}"),
            None => message.to_owned(),
        };
        if !words.trim().is_empty() {
            parts.push(CellOutput {
                label: "error",
                text: words,
                hue: Hue::Fail,
            });
        }
    }
    let printed: Vec<String> = list_of(report, "outputs")
        .iter()
        .filter(|item| str_of(item, "type") == Some("text"))
        .filter_map(|item| str_of(item, "text"))
        .map(|text| {
            if open {
                text.trim_end().to_owned()
            } else {
                compact(text)
            }
        })
        .collect();
    if !printed.is_empty() {
        parts.push(CellOutput {
            label: "printed",
            text: printed.join("\n"),
            hue: Hue::Neutral,
        });
    }
    if let Some(value) = str_of(report, "value") {
        parts.push(CellOutput {
            label: "returned",
            text: if open {
                value.to_owned()
            } else {
                compact(value)
            },
            hue: Hue::Neutral,
        });
    }
    if open {
        return parts;
    }
    let mut room = OUTPUT_ROWS;
    for part in &mut parts {
        let rows: Vec<&str> = part.text.lines().collect();
        if rows.len() > room {
            let more = rows.len() - room;
            let mut kept = rows[..room].join("\n");
            kept.push_str(&format!(
                "\n… {}",
                plural(more as u64, "more row", "more rows")
            ));
            part.text = kept;
            room = 0;
        } else {
            room -= rows.len();
        }
    }
    parts
}

/// The tools a cell called, in first-call order (`read ×3, bash`), at most three named.
fn tool_summary(children: &[Value], omitted: &Value) -> String {
    let mut tally: Vec<(String, u64)> = Vec::new();
    let mut add = |tool: &str, n: u64| {
        let label = tool.trim_start_matches("corral_").replace('_', " ");
        match tally.iter_mut().find(|(known, _)| *known == label) {
            Some((_, count)) => *count += n,
            None => tally.push((label, n)),
        }
    };
    for child in children {
        add(str_of(child, "tool").unwrap_or("tool"), 1);
    }
    if let Value::Object(omitted) = omitted {
        for (tool, n) in omitted {
            add(tool, n.as_u64().unwrap_or(0));
        }
    }
    let named: Vec<String> = tally
        .iter()
        .map(|(tool, n)| {
            if *n > 1 {
                format!("{tool} ×{n}")
            } else {
                tool.clone()
            }
        })
        .collect();
    if named.len() > 3 {
        format!("{} +{}", named[..3].join(", "), named.len() - 3)
    } else {
        named.join(", ")
    }
}

fn status_word(status: &str) -> Option<&'static str> {
    match status {
        "error" => Some("failed"),
        "cancelled" => Some("cancelled"),
        "timeout" => Some("timed out"),
        "reset" => Some("stopped by a reset"),
        "closed" => Some("stopped when the session closed"),
        _ => None,
    }
}

/// A `codemode` call as its cell.
pub(super) fn cell(
    call: &Call<'_>,
    call_id: &str,
    key: &str,
    ms: Option<u64>,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> CodeCell {
    let open = cx.open(key);
    let reports = list_of(&call.details, "cells");
    let detached_settled = cx
        .journal
        .cells
        .get(call_id)
        .and_then(|entry| entry.get("report"));
    let control = if let Some(cell) = call.arg("cancel") {
        let report = reports.first();
        let after = report
            .and_then(|report| count_of(report, "durationMs"))
            .and_then(elapsed)
            .map(|time| format!("stopped after {time}"));
        let calls = report
            .and_then(|report| report.get("calls"))
            .and_then(|calls| count_of(calls, "total"))
            .map(|total| plural(total, "call", "calls"));
        let facts: Vec<String> = after.into_iter().chain(calls).collect();
        Some(if facts.is_empty() {
            format!("cancel {}", cell_label(cell))
        } else {
            format!("cancel {} · {}", cell_label(cell), facts.join(", "))
        })
    } else if call.args.get("reset").and_then(Value::as_bool) == Some(true)
        && call.arg("code").is_none()
    {
        Some("reset kernel · variables discarded".to_owned())
    } else {
        None
    };
    let report = detached_settled.or_else(|| reports.last());
    let status = report.and_then(|report| str_of(report, "status"));
    let settled = status.is_some_and(|status| FINAL.contains(&status));
    let detached = !settled
        && (status == Some("detached")
            || (status == Some("running") && matches!(call.state, CallState::Done)));
    let state = match (status, call.state) {
        (_, CallState::Writing) => ToolState::Writing,
        (Some("ok"), _) => ToolState::Ok,
        (Some(status), _) if settled && status != "ok" => ToolState::Failed,
        (Some(_), _) => ToolState::Running,
        (None, CallState::Done) => ToolState::Ok,
        (None, CallState::Failed) => ToolState::Failed,
        (None, CallState::NotRun) => ToolState::Waiting,
        (None, CallState::Running) => ToolState::Running,
    };
    let children_values: &[Value] = report
        .map(|report| {
            if report.get("children").and_then(Value::as_array).is_some() {
                list_of(report, "children")
            } else {
                list_of(report, "failures")
            }
        })
        .unwrap_or(&[]);
    let omitted = report
        .and_then(|report| report.get("childrenOmitted"))
        .unwrap_or(&Value::Null);
    let rows: Vec<Tool> = children_values
        .iter()
        .map(|outcome| child(outcome, settled, cx))
        .collect();
    let recorded_counts = report.and_then(|report| report.get("calls"));
    let all_recorded = report.is_some_and(|report| report.get("children").is_some())
        && omitted.as_object().is_none_or(|omitted| omitted.is_empty());
    let count = |wanted: ToolState| rows.iter().filter(|row| row.state == wanted).count() as u64;
    let (total, ok, failed, running, unknown) = if all_recorded {
        (
            rows.len() as u64,
            count(ToolState::Ok),
            count(ToolState::Failed),
            count(ToolState::Running),
            count(ToolState::Unknown),
        )
    } else {
        let field = |name: &str| {
            recorded_counts
                .and_then(|calls| count_of(calls, name))
                .unwrap_or(0)
        };
        (
            field("total"),
            field("ok"),
            field("failed"),
            if settled { 0 } else { field("running") },
            if settled { field("running") } else { 0 },
        )
    };
    let tally = [
        (ToolState::Ok, ok),
        (ToolState::Running, running),
        (ToolState::Unknown, unknown),
        (ToolState::Failed, failed),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .collect();
    let calls = control.unwrap_or_else(|| {
        if report.is_none() {
            String::new()
        } else if total == 0 {
            "no tool calls".to_owned()
        } else {
            plural(total, "call", "calls")
        }
    });
    let duration = report
        .and_then(|report| count_of(report, "durationMs"))
        .or(ms);
    let code = call.arg("code").unwrap_or_default();
    CodeCell {
        state,
        calls,
        tally,
        tools: tool_summary(children_values, omitted),
        ended: status.and_then(status_word).map(str::to_owned),
        label: if detached {
            Some("in background".to_owned())
        } else {
            report
                .and_then(|report| str_of(report, "cell"))
                .map(cell_label)
        },
        time: duration.and_then(elapsed),
        note: if detached {
            Some("the agent continues; this row updates when the cell settles".to_owned())
        } else {
            detached_settled.map(|_| "settled in background".to_owned())
        },
        children: folded(rows, open),
        output: report
            .map(|report| outputs(report, open))
            .unwrap_or_default(),
        writing: if state == ToolState::Writing {
            let lines: Vec<&str> = code.lines().collect();
            lines[lines.len().saturating_sub(WRITING_TAIL)..]
                .iter()
                .map(|line| (*line).to_owned())
                .collect()
        } else {
            Vec::new()
        },
        source: (open && !code.trim().is_empty()).then(|| code.to_owned()),
        open,
        more: rest.more(cx),
        action: Some(rest.toggle(key, cx)),
    }
}

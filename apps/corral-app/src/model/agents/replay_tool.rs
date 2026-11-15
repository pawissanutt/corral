//! Tool rows in one grammar, as the terminal draws them: the state mark, the tool, what it
//! acted on and how it ended on one quiet line; a failure keeps its last output lines while
//! folded. Opened, a row shows its diff or written content with the review's diff rows, its
//! arguments, its output and the complete recorded call and result.

use corral_protocol::detail::{SessionItem, SessionToolState};

use crate::model::board::{LineKind, Mark};
use crate::model::review::{NumberedLine, ParsedHunk, parse_patch};
use crate::model::session_feed::{LiveTool, LiveToolState};

use super::replay::{Cx, Rest, first_line, lines};
use super::{Action, LineClass, More, Section, Tool, line};

/// Output rows a folded failed call keeps, and a running call's streamed tail.
const FAILED_ROWS: usize = 3;
const RUNNING_ROWS: usize = 2;

/// Arguments whose value says what a call acted on, in the order they are looked for.
const TARGETS: [&str; 9] = [
    "command",
    "path",
    "file_path",
    "pattern",
    "query",
    "url",
    "target",
    "id",
    "prompt",
];

/// A call's own time: hidden below 100 ms, then `0.4s`, `12s`, `1m 12s`, `1h 02m`.
pub(super) fn elapsed(ms: u64) -> Option<String> {
    let seconds = ms / 1000;
    match ms {
        0..100 => None,
        100..10_000 => Some(format!("{:.1}s", ms as f64 / 1000.0)),
        10_000..60_000 => Some(format!("{seconds}s")),
        60_000..3_600_000 => Some(format!("{}m {:02}s", seconds / 60, seconds % 60)),
        _ => Some(format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60)),
    }
}

/// JSON text pretty-printed; text that does not parse (a clipped preview) stays as it is.
pub(super) fn pretty(json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| json.to_owned())
}

/// A tool name read as words (`job_wait` → `job wait`).
fn words(name: &str) -> String {
    if name.trim().is_empty() {
        "tool".to_owned()
    } else {
        name.replace('_', " ")
    }
}

/// The last `rows` non-empty lines of `text`, muted.
fn last_rows(text: &str, rows: usize) -> Vec<super::Line> {
    let kept: Vec<&str> = text.lines().filter(|row| !row.trim().is_empty()).collect();
    kept[kept.len().saturating_sub(rows)..]
        .iter()
        .map(|row| line(row, LineClass::Dim))
        .collect()
}

/// Recorded text under a label, unless there is none.
fn text(label: &str, body: &str) -> Option<Section> {
    (!body.trim().is_empty()).then(|| Section::Text {
        label: label.to_owned(),
        text: body.to_owned(),
    })
}

/// A changed file as diff rows: a unified patch as served, or the terminal's display diff
/// (`+12 added`, `-12 removed`, ` 12 kept`, a numberless row between hunks).
fn diff(label: &str, body: &str) -> Option<Section> {
    let unified = body.lines().any(|row| row.starts_with("@@"));
    let (hunks, notes) = if unified {
        let patch = parse_patch(body, true);
        (patch.hunks, patch.notes)
    } else {
        (display_diff(body), Vec::new())
    };
    if hunks.iter().all(|hunk| hunk.lines.is_empty()) {
        return text(label, body);
    }
    Some(Section::Diff {
        label: label.to_owned(),
        hunks,
        notes,
    })
}

fn display_diff(body: &str) -> Vec<ParsedHunk> {
    let mut hunks = vec![ParsedHunk {
        header: String::new(),
        lines: Vec::new(),
    }];
    // New line numbers of kept rows: their old number moved by every change shown before them.
    let mut shift: i64 = 0;
    for row in body.lines() {
        let (kind, sign, rest) = match row.as_bytes().first() {
            Some(b'+') => (LineKind::Add, "+", &row[1..]),
            Some(b'-') => (LineKind::Del, "-", &row[1..]),
            Some(b' ') => (LineKind::Context, "", &row[1..]),
            _ => continue,
        };
        let digits = rest.trim_start();
        let end = digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
        let Ok(number) = digits[..end].parse::<u32>() else {
            if hunks.last().is_some_and(|hunk| !hunk.lines.is_empty()) {
                hunks.push(ParsedHunk {
                    header: "⋯".to_owned(),
                    lines: Vec::new(),
                });
            }
            continue;
        };
        let code = &digits[end..];
        let code = code.strip_prefix(' ').unwrap_or(code);
        let (old, new) = match kind {
            LineKind::Add => {
                shift += 1;
                (None, Some(number))
            }
            LineKind::Del => {
                shift -= 1;
                (Some(number), None)
            }
            LineKind::Context => (Some(number), u32::try_from(i64::from(number) + shift).ok()),
        };
        if let Some(hunk) = hunks.last_mut() {
            hunk.lines.push(NumberedLine {
                kind,
                old,
                new,
                sign,
                text: code.to_owned(),
                highlight: None,
            });
        }
    }
    hunks
}

/// Written content as added rows numbered from one.
fn written(label: &str, body: &str) -> Option<Section> {
    if body.is_empty() {
        return None;
    }
    let rows = body
        .lines()
        .zip(1u32..)
        .map(|(row, number)| NumberedLine {
            kind: LineKind::Add,
            old: None,
            new: Some(number),
            sign: "+",
            text: row.to_owned(),
            highlight: None,
        })
        .collect();
    Some(Section::Diff {
        label: label.to_owned(),
        hunks: vec![ParsedHunk {
            header: String::new(),
            lines: rows,
        }],
        notes: Vec::new(),
    })
}

/// A recorded tool call, or the half of one this page holds.
pub(super) fn call(item: &SessionItem, key: &str, rest: Rest<'_>, cx: Cx<'_>) -> Option<Tool> {
    let SessionItem::ToolCall {
        call_id,
        name,
        arguments,
        summary,
        state,
        paired,
        call,
        output,
        result,
        elapsed_ms,
        diff: changed,
        content,
    } = item
    else {
        return None;
    };
    // A call keeps its row's state across pages and refreshes by its id.
    let key = if call_id.is_empty() {
        key.to_owned()
    } else {
        format!("call:{call_id}")
    };
    let (mark, outcome, failed) = match state {
        SessionToolState::Running => (Mark::Active, "running".to_owned(), false),
        SessionToolState::Done => (
            Mark::Done,
            elapsed_ms
                .and_then(elapsed)
                .unwrap_or_else(|| "done".to_owned()),
            false,
        ),
        SessionToolState::Failed => (Mark::Fail, "failed".to_owned(), true),
        SessionToolState::NotRun => (Mark::Next, "not run".to_owned(), false),
    };
    let note = if *paired {
        None
    } else if arguments.trim().is_empty() {
        Some("its call is not on this page".to_owned())
    } else if result.is_none() {
        Some("no result recorded yet".to_owned())
    } else {
        None
    };
    let output = output.as_deref().unwrap_or_default();
    let open = cx.open(&key);
    let body: Vec<Section> = [
        changed.as_deref().and_then(|body| diff("diff", body)),
        content.as_deref().and_then(|body| written("content", body)),
        text("arguments", &pretty(arguments)),
        text("output", output),
        call.as_deref()
            .and_then(|body| text("recorded call", &pretty(body))),
        result
            .as_deref()
            .and_then(|body| text("recorded result", &pretty(body))),
    ]
    .into_iter()
    .flatten()
    .collect();
    let count = output.lines().count();
    Some(Tool {
        mark,
        name: words(name),
        summary: {
            let target = target(arguments);
            if target.is_empty() {
                target_summary(summary)
            } else {
                target
            }
        },
        outcome,
        failed,
        note,
        tail: if failed && !open {
            last_rows(output, FAILED_ROWS)
        } else {
            Vec::new()
        },
        hidden: (count > 0 && !open).then(|| lines(count)),
        open,
        action: (!body.is_empty() || !matches!(rest, Rest::Whole)).then(|| rest.toggle(&key, cx)),
        body: if open { body } else { Vec::new() },
        more: rest.more(cx),
    })
}

/// The owner's own `!command`: a row in the bash grammar marked as the owner's, with its exit
/// (an unknown exit is never 0), and whether it was cancelled, cut or kept from the agent.
pub(super) fn bash(item: &SessionItem, key: &str, rest: Rest<'_>, cx: Cx<'_>) -> Option<Tool> {
    let SessionItem::BashExecution {
        command,
        output,
        exit_code,
        cancelled,
        truncated,
        exclude_from_context,
        full_output_path,
    } = item
    else {
        return None;
    };
    let (first, more_rows) = first_line(command);
    let (mark, mut outcome, failed) = if *cancelled {
        (Mark::Fail, "cancelled".to_owned(), true)
    } else {
        match exit_code {
            Some(0) => (Mark::Done, "exit 0".to_owned(), false),
            Some(code) => (Mark::Fail, format!("exit {code}"), true),
            None => (Mark::Next, "exit unknown".to_owned(), false),
        }
    };
    if *truncated {
        outcome.push_str(" · output cut");
    }
    let open = cx.open(key);
    let body: Vec<Section> = [
        text("command", command),
        text("output", output),
        full_output_path
            .as_deref()
            .and_then(|path| text("full output saved at", path)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let count = output.lines().count();
    Some(Tool {
        mark,
        name: "bash".to_owned(),
        summary: if more_rows > 0 {
            format!("{first} (+{})", lines(more_rows))
        } else {
            first.to_owned()
        },
        outcome,
        failed,
        note: Some(if *exclude_from_context {
            "you · not sent to the agent".to_owned()
        } else {
            "you".to_owned()
        }),
        tail: if failed && !open {
            last_rows(output, FAILED_ROWS)
        } else {
            Vec::new()
        },
        hidden: (count > 0 && !open).then(|| lines(count)),
        open,
        action: Some(rest.toggle(key, cx)),
        body: if open { body } else { Vec::new() },
        more: rest.more(cx),
    })
}

/// What a call's arguments say it acts on: the first target-like string, its first line.
fn target(args: &str) -> String {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str::<serde_json::Value>(args)
    else {
        return String::new();
    };
    let value = TARGETS
        .iter()
        .find_map(|name| fields.get(*name))
        .or_else(|| fields.values().next());
    match value {
        Some(serde_json::Value::String(value)) => first_line(value).0.to_owned(),
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Some(serde_json::Value::Object(fields)) => format!("{} fields", fields.len()),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn target_summary(summary: &str) -> String {
    if summary.trim_start().starts_with('{') {
        target(summary)
    } else {
        first_line(summary).0.to_owned()
    }
}

/// A call of the live turn, in its state, with the tail of its output so far.
pub(super) fn live(tool: &LiveTool, cx: Cx<'_>) -> Tool {
    // The same key as the recorded call, so an opened row stays open once it is recorded.
    let key = format!("call:{}", tool.call);
    let time = tool.elapsed_ms.and_then(elapsed);
    let (mark, outcome, failed) = match tool.state {
        LiveToolState::Running => (
            Mark::Active,
            time.map_or_else(|| "running".to_owned(), |time| format!("running · {time}")),
            false,
        ),
        LiveToolState::Done => (Mark::Done, time.unwrap_or_else(|| "done".to_owned()), false),
        LiveToolState::Failed => (Mark::Fail, "failed".to_owned(), true),
    };
    let partial = tool.partial.as_deref().unwrap_or_default();
    let args = tool.args.as_deref().unwrap_or_default();
    let open = cx.open(&key);
    let body: Vec<Section> = [
        text(
            if tool.args_truncated {
                "arguments (cut at the stream's bound)"
            } else {
                "arguments"
            },
            &pretty(args),
        ),
        text(
            if tool.state == LiveToolState::Running {
                "output so far"
            } else {
                "output"
            },
            partial,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    let tail = match (tool.state, open) {
        (_, true) => Vec::new(),
        (LiveToolState::Running, false) => last_rows(partial, RUNNING_ROWS),
        (LiveToolState::Failed, false) => last_rows(partial, FAILED_ROWS),
        (LiveToolState::Done, false) => Vec::new(),
    };
    Tool {
        mark,
        name: words(&tool.tool),
        summary: target(args),
        outcome,
        failed,
        note: None,
        tail,
        hidden: None,
        open,
        action: (!body.is_empty()).then(|| Action::Expand { key, fetch: None }),
        body: if open { body } else { Vec::new() },
        more: More::Whole,
    }
}

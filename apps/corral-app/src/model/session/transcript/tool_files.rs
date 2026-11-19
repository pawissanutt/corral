//! Rows for Pi's file tools, as `tool-lines-files.ts` words them: `read <path>:<from>-<to> · 27
//! lines` (`20 of 300 lines · more`, `first line too long`), `edit <path> · +2 −2 in 2 hunks`,
//! `write <path> · 40 lines · 1.2 KB`, `grep "<pattern>" in <path> (<glob>) · 35+ matches · 7
//! files` or `no matches`, `find` and `ls` with counts (`no files`, `empty`). File errors read in
//! words (`not found`, `permission denied`, `is a directory`). Opened, a row shows the file text
//! or the rendered diff.

use serde_json::Value;

use crate::model::hue::Hue;
use crate::model::session::{Action, ChangesAction};

use super::tool::{
    Call, CallState, Row, ToolView, View, count_of, diff, plural, rows_of, str_of, text, written,
};

/// Rows a streaming `write` keeps.
const WRITE_TAIL_ROWS: usize = 8;
/// A failed call shows at most this many reason rows.
const REASON_ROWS: usize = 3;

pub(super) fn present(call: &Call<'_>) -> Option<Row> {
    Some(match call.name {
        "read" => read(call),
        "edit" => edit(call),
        "write" => write(call),
        "grep" => search(call, "grep"),
        "find" => search(call, "find"),
        "ls" => ls(call),
        _ => return None,
    })
}

fn path_of(call: &Call<'_>) -> String {
    call.arg("path")
        .or_else(|| call.arg("file_path"))
        .map(|path| call.path(path))
        .unwrap_or_default()
}

/// The Changes link of an edit or write row: the tab opened at the file the call names (the
/// tab resolves the path against its list), else the tab alone.
fn changes_at(call: &Call<'_>) -> ToolView {
    match call
        .arg("path")
        .or_else(|| call.arg("file_path"))
        .filter(|path| !path.is_empty())
    {
        Some(path) => ToolView {
            label: "Changes",
            action: Action::Changes(ChangesAction::Open {
                session: call.session.clone(),
                path: path.to_owned(),
            }),
        },
        None => call.view(View::Changes),
    }
}

/// A byte count as the file rows say it: `820 B`, `1.2 KB`, `3.4 MB`.
fn size(bytes: usize) -> String {
    let value = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", value / 1024.0)
    } else {
        format!("{:.1} MB", value / 1024.0 / 1024.0)
    }
}

/// A result's text without the tool's trailing `[...]` notice block, and that notice.
fn without_notice(value: &str) -> (&str, Option<&str>) {
    let end = value.trim_end();
    match end.rfind("\n\n[") {
        Some(at) if end.ends_with(']') => (&end[..at], Some(&end[at + 3..end.len() - 1])),
        _ => (value, None),
    }
}

/// A failure: a known file error reads in words with the whole reason below, any other its
/// first line with the rest below.
fn failure(row: Row, text: &str) -> Row {
    let reason: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let first = reason.first().copied().unwrap_or("failed").trim();
    let short = [
        ("ENOENT", "not found"),
        ("ENOTDIR", "not a directory"),
        ("EISDIR", "is a directory"),
        ("EACCES", "permission denied"),
        ("EPERM", "permission denied"),
        ("EEXIST", "already exists"),
    ]
    .into_iter()
    .find_map(|(code, words)| first.contains(code).then_some(words));
    let skip = usize::from(short.is_none());
    let body = reason
        .iter()
        .skip(skip)
        .take(REASON_ROWS)
        .map(|line| (*line).to_owned())
        .collect();
    let mut row = row.fail(short.unwrap_or(first));
    row.body = body;
    row
}

/// `:12-40` from the read's offset and limit.
fn range(args: &Value) -> String {
    let offset = count_of(args, "offset");
    let limit = count_of(args, "limit");
    if offset.is_none() && limit.is_none() {
        return String::new();
    }
    let start = offset.unwrap_or(1);
    match limit {
        Some(limit) => format!(":{start}-{}", start + limit.max(1) - 1),
        None => format!(":{start}"),
    }
}

/// `A of n lines · more` from a `Showing lines a-b of n` notice.
fn showing(notice: &str) -> Option<String> {
    let rest = notice.strip_prefix("Showing lines ")?;
    let (span, rest) = rest.split_once(" of ")?;
    let (from, to) = span.split_once('-')?;
    let total: u64 = rest
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    let shown = to.parse::<u64>().ok()? + 1 - from.parse::<u64>().ok()?;
    Some(format!(
        "{shown} of {} · more",
        plural(total, "line", "lines")
    ))
}

fn read(call: &Call<'_>) -> Row {
    let path = path_of(call);
    let row = Row::new(call, "read").target(format!("{path}{}", range(&call.args)));
    match call.state {
        CallState::Failed => return failure(row, call.text),
        CallState::Done => {}
        _ => return row,
    }
    if call
        .details
        .get("truncation")
        .and_then(|truncation| truncation.get("firstLineExceedsLimit"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        let mut row = row.ok("first line too long");
        row.hue = Hue::Held;
        return row;
    }
    let (body, notice) = without_notice(call.text);
    let mut row = if let Some(lines) = call.lines {
        row.ok(plural(lines, "line", "lines"))
    } else {
        let count = rows_of(body).len() as u64;
        let more = notice.and_then(|notice| {
            let (lines, _) = notice.split_once(" more lines in file")?;
            lines.parse::<u64>().ok()
        });
        let outcome = notice
            .and_then(showing)
            .or_else(|| {
                more.map(|more| format!("{} · {more} more", plural(count, "line", "lines")))
            })
            .unwrap_or_else(|| plural(count, "line", "lines"));
        row.ok(outcome)
    };
    row.expanded.extend(text("file", body));
    row
}

/// `+a −d in n hunks` from a unified patch, or from the display diff.
fn diff_stat(patch: Option<&str>, display: Option<&str>) -> Option<String> {
    let (mut added, mut removed, mut hunks) = (0u64, 0u64, 0u64);
    if let Some(patch) = patch {
        for line in patch.lines() {
            if line.starts_with("@@") {
                hunks += 1;
            } else if line.starts_with('+') && !line.starts_with("+++") {
                added += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                removed += 1;
            }
        }
    } else {
        let mut changing = false;
        for line in display?.lines() {
            let change = line.starts_with('+') || line.starts_with('-');
            if change && !changing {
                hunks += 1;
            }
            changing = change;
            if line.starts_with('+') {
                added += 1;
            } else if line.starts_with('-') {
                removed += 1;
            }
        }
    }
    Some(format!(
        "+{added} \u{2212}{removed} in {}",
        plural(hunks, "hunk", "hunks")
    ))
}

fn edit(call: &Call<'_>) -> Row {
    let edits = call
        .args
        .get("edits")
        .and_then(Value::as_array)
        .map_or(1, Vec::len) as u64;
    let mut row = Row::new(call, "edit").target(path_of(call));
    row.view = Some(changes_at(call));
    match call.state {
        CallState::Failed => failure(row, call.text),
        CallState::Done => {
            let patch = str_of(&call.details, "patch");
            let display = str_of(&call.details, "diff");
            if let Some(shown) = display.or(patch).and_then(|body| diff("diff", body)) {
                row.expanded.push(shown);
            }
            match diff_stat(patch, display) {
                Some(stat) => row.ok(stat),
                None => row,
            }
        }
        _ if edits > 1 => row.says(plural(edits, "edit", "edits")),
        _ => row,
    }
}

fn write(call: &Call<'_>) -> Row {
    let content = call.arg("content").unwrap_or_default();
    let all = rows_of(content);
    let stat = format!(
        "{} · {}",
        plural(all.len() as u64, "line", "lines"),
        size(content.len())
    );
    let mut row = Row::new(call, "write").target(path_of(call));
    row.view = Some(changes_at(call));
    match call.state {
        CallState::Failed => failure(row, call.text),
        CallState::Writing | CallState::Running if all.len() >= 2 => {
            row.body = all[all.len().saturating_sub(WRITE_TAIL_ROWS)..]
                .iter()
                .map(|line| (*line).to_owned())
                .collect();
            row.says(stat)
        }
        _ => {
            row.expanded.extend(written("content", content));
            if content.is_empty() {
                row
            } else {
                row.says(stat)
            }
        }
    }
}

/// `n+ things` when the tool stopped at its limit, else the exact count.
fn counted(count: u64, limit: Option<u64>, one: &str, many: &str) -> String {
    match limit {
        Some(limit) => format!("{limit}+ {many}"),
        None => plural(count, one, many),
    }
}

fn scope(call: &Call<'_>) -> String {
    call.arg("path")
        .map(|path| call.path(path))
        .unwrap_or_else(|| ".".to_owned())
}

/// The file of a `path:line: text` match row.
fn matched(row: &str) -> Option<&str> {
    let mut at = 0;
    while let Some(found) = row[at..].find(':') {
        let colon = at + found;
        let rest = &row[colon + 1..];
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits > 0 && rest[digits..].starts_with(": ") {
            return Some(&row[..colon]);
        }
        at = colon + 1;
    }
    None
}

fn search(call: &Call<'_>, verb: &'static str) -> Row {
    let pattern = call.arg("pattern").unwrap_or_default();
    let glob = call
        .arg("glob")
        .map(|glob| format!(" ({glob})"))
        .unwrap_or_default();
    let target = match (verb, pattern.is_empty()) {
        (_, true) => format!("in {}{glob}", scope(call)),
        ("grep", false) => format!("\"{pattern}\" in {}{glob}", scope(call)),
        _ => format!("{pattern} in {}{glob}", scope(call)),
    };
    let row = Row::new(call, verb).target(target);
    match call.state {
        CallState::Failed => return failure(row, call.text),
        CallState::Done => {}
        _ => return row,
    }
    let (body, _) = without_notice(call.text);
    let rows = rows_of(body);
    let empty = rows.is_empty()
        || (rows.len() == 1
            && matches!(
                rows[0].trim(),
                "No matches found" | "No files found matching pattern"
            ));
    if empty {
        return row.ok(if verb == "grep" {
            "no matches"
        } else {
            "no files"
        });
    }
    let mut row = if verb == "grep" {
        let limit = count_of(&call.details, "matchLimitReached");
        let mut files: Vec<&str> = Vec::new();
        let mut matches = 0;
        for line in &rows {
            if let Some(file) = matched(line) {
                matches += 1;
                if !files.contains(&file) {
                    files.push(file);
                }
            }
        }
        let mut row = row.ok(format!(
            "{} · {}",
            counted(matches, limit, "match", "matches"),
            plural(files.len() as u64, "file", "files")
        ));
        row.hidden = Some(plural(rows.len() as u64, "line", "lines"));
        row
    } else {
        let limit = count_of(&call.details, "resultLimitReached");
        let count = call.lines.unwrap_or(rows.len() as u64);
        row.ok(counted(count, limit, "file", "files"))
    };
    row.expanded.extend(text("output", &rows.join("\n")));
    row
}

fn ls(call: &Call<'_>) -> Row {
    let row = Row::new(call, "ls").target(scope(call));
    match call.state {
        CallState::Failed => return failure(row, call.text),
        CallState::Done => {}
        _ => return row,
    }
    let (body, _) = without_notice(call.text);
    let rows = rows_of(body);
    if rows.is_empty() || (rows.len() == 1 && rows[0].trim() == "(empty directory)") {
        return row.ok("empty");
    }
    let limit = count_of(&call.details, "entryLimitReached");
    let count = call.lines.unwrap_or(rows.len() as u64);
    let mut row = row.ok(counted(count, limit, "entry", "entries"));
    row.expanded.extend(text("output", &rows.join("\n")));
    row
}

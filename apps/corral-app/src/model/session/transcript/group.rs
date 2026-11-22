//! Runs of one kind, as the terminal UI folds them (`tool-group.ts`): consecutive settled
//! successful rows of one tool and verb (plan updates across their verbs), with nothing visible
//! between them, fold into one row in the same grammar with the total time on the right and the
//! newest view kept. A kind with a summary of its own folds from two rows (`read src/a.rs · 3
//! reads`, `read ×5 · recovery.rs, evaluate.rs, …`, `Plan 5/7 · done: …`, `Attached 5 artifacts ·
//! report, 3 logs`), any other from three as `✓ <verb> ×n · <newest outcome>`. A running,
//! failed, waiting, unknown, needs-you or opened row never folds and ends the run.

use crate::model::hue::Hue;

use super::replay::Cx;
use super::tool::{ToolState, elapsed, plural};
use super::{Action, MessageItem, More, Tool};

/// Rows that fold under the generic summary, and under a summary of their own.
const GENERIC_MIN: usize = 3;
const SUMMARY_MIN: usize = 2;
/// Names a group's outcome lists before `… n more`.
const NAMED: usize = 3;

/// What a row folds by.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fold {
    /// The row's own key; a group's key normally comes from its first row.
    pub key: String,
    /// The tool's recorded name.
    pub tool: String,
    /// A plan update's operation, an artifact's kind.
    pub facet: Option<String>,
    pub ms: u64,
}

/// A run of settled calls of one kind: one row, and its rows while open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub row: Tool,
    pub rows: Vec<Tool>,
    pub open: bool,
    /// Number of input rows represented by the group, even while it is closed.
    pub(super) count: usize,
}

/// How a tool's rows fold and are summarised.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Summary {
    /// `<verb> ×n · <newest outcome>`.
    Generic,
    /// One target read in parts, else the names read.
    Paged,
    /// `<verb> ×n · <file names>`.
    Files,
    /// `<verb> ×n · <patterns>`.
    Patterns,
    Plan,
    Attached,
}

fn summary(tool: &str) -> Summary {
    match tool {
        "read" | "edit" | "web_read" => Summary::Paged,
        "write" => Summary::Files,
        "grep" | "find" | "ls" | "web_search" | "tool_search" => Summary::Patterns,
        "corral_todo" => Summary::Plan,
        "corral_artifact" => Summary::Attached,
        _ => Summary::Generic,
    }
}

/// What a row folds with: its tool and verb, or its tool alone for plan updates.
fn kind(tool: &Tool) -> String {
    if summary(&tool.fold.tool) == Summary::Plan {
        tool.fold.tool.clone()
    } else {
        format!("{}\u{0}{}", tool.fold.tool, tool.name)
    }
}

fn foldable(item: &MessageItem) -> Option<&Tool> {
    match item {
        MessageItem::Tool(tool) if tool.state == ToolState::Ok && !tool.open => Some(tool),
        _ => None,
    }
}

/// `items` with every run of one kind folded into a group.
pub(super) fn fold(items: Vec<MessageItem>, cx: Cx<'_>) -> Vec<MessageItem> {
    let mut out = Vec::with_capacity(items.len());
    let mut run: Vec<Tool> = Vec::new();
    for item in items {
        let Some(tool) = foldable(&item) else {
            settle(&mut run, &mut out, cx);
            out.push(item);
            continue;
        };
        if run.first().is_some_and(|first| kind(first) != kind(tool)) {
            settle(&mut run, &mut out, cx);
        }
        if let MessageItem::Tool(tool) = item {
            run.push(tool);
        }
    }
    settle(&mut run, &mut out, cx);
    out
}

/// The run so far, as one group when it is long enough, else its rows.
fn settle(run: &mut Vec<Tool>, out: &mut Vec<MessageItem>, cx: Cx<'_>) {
    let rows = std::mem::take(run);
    let Some(first) = rows.first() else {
        return;
    };
    let min = if summary(&first.fold.tool) == Summary::Generic {
        GENERIC_MIN
    } else {
        SUMMARY_MIN
    };
    if rows.len() < min {
        out.extend(rows.into_iter().map(MessageItem::Tool));
        return;
    }
    let key = group_key(&rows, first, cx);
    let open = cx.open(&key);
    let row = summarised(&rows, key, cx);
    out.push(MessageItem::Group(Group {
        row,
        count: rows.len(),
        rows: if open { rows } else { Vec::new() },
        open,
    }));
}

/// Keep a toggled group's key if a split record gains earlier calls of the same kind.
fn group_key(rows: &[Tool], first: &Tool, cx: Cx<'_>) -> String {
    const PREFIX: &str = "group:";
    let mut key = PREFIX.to_owned();
    if !cx.ui.expanded.is_empty() {
        for row in rows {
            key.push_str(&row.fold.key);
            if cx.ui.expanded.contains(&key) {
                return key;
            }
            key.truncate(PREFIX.len());
        }
    }
    key.push_str(&first.fold.key);
    key
}

/// A file target without its `:from-to` range.
fn whole_file(target: &str) -> &str {
    match target.rsplit_once(':') {
        Some((path, range))
            if !range.is_empty() && range.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            path
        }
        _ => target,
    }
}

fn file_name(target: &str) -> &str {
    let path = whole_file(target);
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// A search's pattern: its quoted part when it has one.
fn pattern(target: &str) -> &str {
    if let Some(rest) = target.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return &target[..end + 2];
    }
    target
}

/// `a.rs, b.rs, c.rs, … 2 more`: the distinct names in order.
fn listing<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let mut distinct: Vec<&str> = Vec::new();
    for name in names.filter(|name| !name.is_empty()) {
        if !distinct.contains(&name) {
            distinct.push(name);
        }
    }
    let shown = distinct
        .iter()
        .take(NAMED)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    match distinct.len().saturating_sub(NAMED) {
        0 => shown,
        more => format!("{shown}, … {more} more"),
    }
}

/// The group row: the run's words in the row grammar, `✓`, the total time, the newest view.
fn summarised(rows: &[Tool], key: String, cx: Cx<'_>) -> Tool {
    let newest = &rows[rows.len() - 1];
    let count = rows.len() as u64;
    let times = format!("×{count}");
    let (verb, target, outcome) = match summary(&newest.fold.tool) {
        Summary::Generic => (newest.name.clone(), times, newest.outcome.clone()),
        Summary::Paged => {
            let first = whole_file(&rows[0].summary);
            if rows.iter().all(|row| whole_file(&row.summary) == first) && !first.is_empty() {
                let unit = if newest.fold.tool == "edit" {
                    ("edit", "edits")
                } else {
                    ("read", "reads")
                };
                (
                    newest.name.clone(),
                    first.to_owned(),
                    plural(count, unit.0, unit.1),
                )
            } else {
                let names = listing(rows.iter().map(|row| file_name(&row.summary)));
                (newest.name.clone(), times, names)
            }
        }
        Summary::Files => (
            newest.name.clone(),
            times,
            listing(rows.iter().map(|row| file_name(&row.summary))),
        ),
        Summary::Patterns => (
            newest.name.clone(),
            times,
            listing(rows.iter().map(|row| pattern(&row.summary))),
        ),
        Summary::Plan => plan(rows),
        Summary::Attached => attached(rows),
    };
    let ms: u64 = rows.iter().map(|row| row.fold.ms).sum();
    Tool {
        state: ToolState::Ok,
        name: verb,
        summary: target,
        outcome,
        outcome_hue: Hue::Neutral,
        detail: None,
        note: None,
        hidden: None,
        time: elapsed(ms),
        view: rows.iter().rev().find_map(|row| row.view.clone()),
        tail: Vec::new(),
        open: cx.open(&key),
        body: Vec::new(),
        more: More::Whole,
        action: Some(Action::Expand {
            session: cx.id.clone(),
            key: key.clone(),
            fetch: None,
        }),
        fold: Fold {
            key,
            tool: newest.fold.tool.clone(),
            facet: None,
            ms,
        },
    }
}

/// `Plan 5/7 · done: Add regression · started: Implement verification`.
fn plan(rows: &[Tool]) -> (String, String, String) {
    let position = rows.iter().rev().find_map(|row| {
        row.outcome
            .split(" · ")
            .find_map(|part| part.strip_prefix("plan "))
            .map(str::to_owned)
    });
    let mut by_word: Vec<(String, Vec<String>)> = Vec::new();
    for row in rows {
        let word = match row.fold.facet.as_deref() {
            Some("init") => "opened",
            Some("append") => "added to",
            Some("start") => "started",
            Some("done") => "done",
            Some("drop") => "dropped",
            _ => "updated",
        };
        let at = match by_word.iter().position(|(known, _)| known == word) {
            Some(at) => at,
            None => {
                by_word.push((word.to_owned(), Vec::new()));
                by_word.len() - 1
            }
        };
        let labels = &mut by_word[at].1;
        if !row.summary.is_empty() && !labels.contains(&row.summary) {
            labels.push(row.summary.clone());
        }
    }
    let outcome = by_word
        .into_iter()
        .map(|(word, labels)| {
            if labels.is_empty() {
                word
            } else {
                format!("{word}: {}", labels.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    (
        "Plan".to_owned(),
        position.unwrap_or_else(|| plural(rows.len() as u64, "update", "updates")),
        outcome,
    )
}

/// `Attached 5 artifacts · report, 3 logs, screenshot`.
fn attached(rows: &[Tool]) -> (String, String, String) {
    let mut kinds: Vec<(String, u64)> = Vec::new();
    for row in rows {
        let kind = row.fold.facet.clone().unwrap_or_else(|| "file".to_owned());
        match kinds.iter_mut().find(|(known, _)| *known == kind) {
            Some((_, count)) => *count += 1,
            None => kinds.push((kind, 1)),
        }
    }
    let outcome = kinds
        .into_iter()
        .map(|(kind, count)| match count {
            1 => kind,
            _ if kind.ends_with('s') => format!("{count} {kind}"),
            _ => format!("{count} {kind}s"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    (
        rows[rows.len() - 1].name.clone(),
        plural(rows.len() as u64, "artifact", "artifacts"),
        outcome,
    )
}

//! Tool rows in one grammar, as the terminal draws them:
//! `<state> <verb> <target> · <outcome> · <detail>   <label> <hidden> <time> <view>`. A pure
//! presenter per tool family turns a call into a [`Row`] (`tool_files`, `tool_jobs`,
//! `tool_corral`, `tool_local`); any other tool reads as its name in words with the one argument
//! that says what it does. Code Mode cells and the build tools draw their own blocks. A failure
//! keeps its reason rows while folded; opened, a row shows the presenter's full view, then the
//! arguments and the complete recorded call and result.

use corral_protocol::detail::{SessionItem, SessionToolState};
use serde_json::Value;

use crate::model::Id;
use crate::model::board::LineKind;
use crate::model::hue::Hue;
use crate::model::review::{NumberedLine, ParsedHunk, parse_patch};
use crate::model::session::Tab;

use super::group::Fold;
use super::journal::Journal;
use super::live::{LiveTool, LiveToolState};
use super::replay::{Cx, Rest, first_line, lines};
use super::{
    Action, LineClass, MessageItem, Section, Tool, build_cards, code_cell, line, tool_corral,
    tool_files, tool_jobs, tool_local,
};

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

/// A running call's streamed tail.
const RUNNING_ROWS: usize = 2;

/// The state a row's glyph says: a spinner while it is written or runs, then `✓`, `✗`, `○`
/// (waiting, queued, not run), `▲` (needs the owner) or `!` (outcome unknown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Writing,
    Running,
    Ok,
    Failed,
    Waiting,
    NeedsYou,
    Unknown,
}

impl ToolState {
    /// The glyph, or none for the spinning states.
    pub fn glyph(self) -> Option<&'static str> {
        match self {
            ToolState::Writing | ToolState::Running => None,
            ToolState::Ok => Some("✓"),
            ToolState::Failed => Some("✗"),
            ToolState::Waiting => Some("○"),
            ToolState::NeedsYou => Some("▲"),
            ToolState::Unknown => Some("!"),
        }
    }

    pub fn hue(self) -> Hue {
        match self {
            ToolState::Writing | ToolState::Running => Hue::Worker,
            ToolState::Ok => Hue::Ok,
            ToolState::Failed => Hue::Fail,
            ToolState::Waiting => Hue::Neutral,
            ToolState::NeedsYou => Hue::You,
            ToolState::Unknown => Hue::Held,
        }
    }

    /// What a screen reader hears for the glyph.
    pub fn label(self) -> &'static str {
        match self {
            ToolState::Writing => "writing",
            ToolState::Running => "running",
            ToolState::Ok => "done",
            ToolState::Failed => "failed",
            ToolState::Waiting => "waiting",
            ToolState::NeedsYou => "needs you",
            ToolState::Unknown => "outcome unknown",
        }
    }
}

/// A view a row opens, named as the tab (`Agents`, `Changes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolView {
    pub label: &'static str,
    pub action: Action,
}

/// The tabs a row can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    Agents,
    Changes,
    Task,
    Proof,
}

/// How a call stands, as its record or the live stream says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CallState {
    Writing,
    Running,
    Done,
    Failed,
    NotRun,
}

/// What a presenter reads of one call.
pub(super) struct Call<'a> {
    pub name: &'a str,
    /// The arguments as an object, or `Null` when they are not here or do not parse.
    pub args: Value,
    /// The structured result's `details`, or `Null`.
    pub details: Value,
    /// The result's text (the output the model read), or the streamed tail.
    pub text: &'a str,
    pub state: CallState,
    pub session: &'a Id,
    pub journal: &'a Journal,
    /// The recorded line count of a Code Mode child, whose text is a clipped preview.
    pub lines: Option<u64>,
}

impl Call<'_> {
    pub fn base(&self) -> ToolState {
        match self.state {
            CallState::Writing => ToolState::Writing,
            CallState::Running => ToolState::Running,
            CallState::Done => ToolState::Ok,
            CallState::Failed => ToolState::Failed,
            CallState::NotRun => ToolState::Waiting,
        }
    }

    pub fn arg(&self, key: &str) -> Option<&str> {
        str_of(&self.args, key)
    }

    /// A path argument relative to the session's directory, else with `~`.
    pub fn path(&self, value: &str) -> String {
        short_path(value, self.journal.cwd.as_deref())
    }

    pub fn view(&self, view: View) -> ToolView {
        let (label, tab) = match view {
            View::Agents => ("Agents", Tab::Agents),
            View::Changes => ("Changes", Tab::Changes),
            View::Task => ("Task", Tab::Task),
            View::Proof => ("Proof", Tab::Proof),
        };
        ToolView {
            label,
            action: Action::Tab {
                session: self.session.clone(),
                tab,
            },
        }
    }

    /// The result as one object: its details when they say anything, else its text as JSON.
    pub fn result(&self) -> Value {
        match &self.details {
            Value::Object(fields) if !fields.is_empty() => self.details.clone(),
            _ => serde_json::from_str(self.text.trim()).unwrap_or(Value::Null),
        }
    }
}

/// What a presenter says about a call.
pub(super) struct Row {
    pub state: ToolState,
    pub verb: String,
    pub target: String,
    pub outcome: String,
    pub hue: Hue,
    pub detail: Option<String>,
    pub right: Option<String>,
    pub hidden: Option<String>,
    /// Muted rows under the folded line.
    pub body: Vec<String>,
    pub view: Option<ToolView>,
    /// The full view, shown first when the row is opened.
    pub expanded: Vec<Section>,
    /// What the row folds by beside its tool and verb (a plan update's step, an artifact's kind).
    pub facet: Option<String>,
}

impl Row {
    pub fn new(call: &Call<'_>, verb: impl Into<String>) -> Row {
        Row {
            state: call.base(),
            verb: verb.into(),
            target: String::new(),
            outcome: String::new(),
            hue: Hue::Neutral,
            detail: None,
            right: None,
            hidden: None,
            body: Vec::new(),
            view: None,
            expanded: Vec::new(),
            facet: None,
        }
    }

    pub fn target(mut self, target: impl Into<String>) -> Row {
        self.target = target.into();
        self
    }

    /// Sets the outcome in the state it has.
    pub fn says(mut self, outcome: impl Into<String>) -> Row {
        self.outcome = outcome.into();
        self
    }

    pub fn ok(mut self, outcome: impl Into<String>) -> Row {
        self.state = ToolState::Ok;
        self.outcome = outcome.into();
        self
    }

    pub fn fail(mut self, outcome: impl Into<String>) -> Row {
        self.state = ToolState::Failed;
        self.hue = Hue::Fail;
        self.outcome = outcome.into();
        self
    }

    pub fn state(mut self, state: ToolState, outcome: impl Into<String>) -> Row {
        self.state = state;
        self.hue = match state {
            ToolState::Failed => Hue::Fail,
            ToolState::NeedsYou | ToolState::Unknown => Hue::Held,
            _ => Hue::Neutral,
        };
        self.outcome = outcome.into();
        self
    }

    /// The reason a failed call gave: its first line as the outcome, the next three under it.
    pub fn failed_with(self, text: &str) -> Row {
        let reason: Vec<&str> = text.lines().filter(|row| !row.trim().is_empty()).collect();
        let first = reason.first().map_or("failed", |row| row.trim());
        let mut row = self.fail(first);
        row.body = reason
            .iter()
            .skip(1)
            .take(3)
            .map(|row| (*row).to_owned())
            .collect();
        row
    }
}

/// A string field, unless empty.
pub(super) fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

pub(super) fn count_of(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

pub(super) fn list_of<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `1 line`, `12 lines`.
pub(super) fn plural(count: u64, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// The rows of `text` without the trailing empty ones.
pub(super) fn rows_of(text: &str) -> Vec<&str> {
    let mut rows: Vec<&str> = text.lines().collect();
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    rows
}

/// The first line of `text` with words in it, trimmed.
pub(super) fn head(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|row| !row.is_empty())
        .unwrap_or_default()
}

/// The last `rows` non-empty lines of `text`.
pub(super) fn last_rows(text: &str, rows: usize) -> Vec<String> {
    let kept: Vec<&str> = text.lines().filter(|row| !row.trim().is_empty()).collect();
    kept[kept.len().saturating_sub(rows)..]
        .iter()
        .map(|row| (*row).to_owned())
        .collect()
}

/// A path relative to `cwd` when inside it, else with `~` for a home directory.
pub(super) fn short_path(path: &str, cwd: Option<&str>) -> String {
    if let Some(cwd) = cwd.map(|cwd| cwd.trim_end_matches('/'))
        && !cwd.is_empty()
    {
        if path == cwd {
            return ".".to_owned();
        }
        if let Some(rest) = path
            .strip_prefix(cwd)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return rest.to_owned();
        }
    }
    for home in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(home)
            && let Some(at) = rest.find('/')
        {
            return format!("~{}", &rest[at..]);
        }
    }
    if let Some(rest) = path.strip_prefix("/root/") {
        return format!("~/{rest}");
    }
    path.to_owned()
}

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
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| json.to_owned())
}

/// A tool name read as words (`job_wait` → `job wait`).
pub(super) fn words(name: &str) -> String {
    if name.trim().is_empty() {
        "tool".to_owned()
    } else {
        name.replace('_', " ")
    }
}

/// Recorded text under a label, unless there is none.
pub(super) fn text(label: &str, body: &str) -> Option<Section> {
    (!body.trim().is_empty()).then(|| Section::Text {
        label: label.to_owned(),
        text: body.to_owned(),
    })
}

/// A changed file as diff rows: a unified patch as served, or the terminal's display diff
/// (`+12 added`, `-12 removed`, ` 12 kept`, a numberless row between hunks).
pub(super) fn diff(label: &str, body: &str) -> Option<Section> {
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
pub(super) fn written(label: &str, body: &str) -> Option<Section> {
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

/// What a call's arguments say it acts on: the first target-like string, its first line.
pub(super) fn target(args: &Value) -> String {
    let Value::Object(fields) = args else {
        return String::new();
    };
    let value = TARGETS
        .iter()
        .find_map(|name| fields.get(*name))
        .or_else(|| fields.values().next());
    match value {
        Some(Value::String(value)) => first_line(value).0.to_owned(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::Object(fields)) => format!("{} fields", fields.len()),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

/// The daemon's one-line summary of a call whose arguments are not on this page.
fn target_summary(summary: &str) -> String {
    if summary.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(summary)
            .map(|value| target(&value))
            .unwrap_or_default()
    } else {
        first_line(summary).0.to_owned()
    }
}

/// A JSON result in words (`3 items`, `status ready · total 4`), never dumped.
fn summarise(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => Some(plural(items.len() as u64, "item", "items")),
        Value::Object(fields) => {
            let words: Vec<String> = fields
                .iter()
                .filter_map(|(key, value)| match value {
                    Value::String(text) if text.len() <= 32 && !text.contains('\n') => {
                        Some(format!("{key} {text}"))
                    }
                    Value::Number(number) => Some(format!("{key} {number}")),
                    Value::Bool(true) => Some(key.replace('_', " ")),
                    Value::Array(items) => Some(format!("{} {key}", items.len())),
                    _ => None,
                })
                .take(2)
                .collect();
            (!words.is_empty()).then(|| words.join(" · "))
        }
        _ => None,
    }
}

/// Any tool no presenter knows: its name in words, the one argument that says what it does,
/// a JSON result summarised in words.
fn fallback(call: &Call<'_>) -> Row {
    let row = Row::new(call, words(call.name)).target(target(&call.args));
    match call.state {
        CallState::Done => {
            let outcome = summarise(&call.result())
                .or_else(|| {
                    let first = head(call.text);
                    (!first.is_empty()).then(|| first.to_owned())
                })
                .unwrap_or_else(|| "done".to_owned());
            row.says(outcome)
        }
        CallState::Failed => row.failed_with(call.text),
        _ => row,
    }
}

/// The row for `call`, by its family's presenter. A call of a stopped turn reads `not run`.
pub(super) fn present(call: &Call<'_>) -> Row {
    let mut row = tool_files::present(call)
        .or_else(|| tool_jobs::present(call))
        .or_else(|| tool_corral::present(call))
        .or_else(|| tool_local::present(call))
        .unwrap_or_else(|| fallback(call));
    if call.state == CallState::NotRun {
        row.state = ToolState::Waiting;
        row.outcome = "not run".to_owned();
        row.hue = Hue::Neutral;
        row.body.clear();
    }
    if row.state == ToolState::Failed && row.outcome.is_empty() {
        row.outcome = "failed".to_owned();
        row.hue = Hue::Fail;
    }
    row
}

/// The recorded parts of a call the opened row keeps after the presenter's view.
pub(super) struct Raw<'a> {
    pub tool: &'a str,
    pub summary: &'a str,
    pub arguments: &'a str,
    pub output: &'a str,
    pub call: Option<&'a str>,
    pub result: Option<&'a str>,
    pub diff: Option<&'a str>,
    pub content: Option<&'a str>,
    pub ms: Option<u64>,
    /// Which half of the call is not on this page.
    pub note: Option<&'static str>,
}

/// The row `row` as drawn: opened or folded by its key, with what it hides and how to open it.
pub(super) fn finish(row: Row, key: &str, raw: Raw<'_>, rest: Rest<'_>, cx: Cx<'_>) -> Tool {
    let open = cx.open(key);
    let mut body = row.expanded;
    if body.is_empty() {
        body.extend(raw.diff.and_then(|patch| diff("diff", patch)));
        body.extend(raw.content.and_then(|content| written("content", content)));
    }
    let shows_output = body
        .iter()
        .any(|section| matches!(section, Section::Text { label, .. } if label == "output"));
    if !shows_output {
        body.extend(text("output", raw.output));
    }
    body.extend(text("arguments", &pretty(raw.arguments)));
    body.extend(
        raw.call
            .and_then(|call| text("recorded call", &pretty(call))),
    );
    body.extend(
        raw.result
            .and_then(|result| text("recorded result", &pretty(result))),
    );
    let count = raw
        .output
        .lines()
        .filter(|row| !row.trim().is_empty())
        .count();
    let hidden = row
        .hidden
        .or_else(|| (count > 1 && !row.outcome.contains(" line")).then(|| lines(count)));
    let mut tail = row.body;
    if tail.is_empty() && row.state == ToolState::Running {
        tail = last_rows(raw.output, RUNNING_ROWS);
    }
    Tool {
        state: row.state,
        name: row.verb,
        summary: if row.target.is_empty() {
            target_summary(raw.summary)
        } else {
            row.target
        },
        outcome: row.outcome,
        outcome_hue: row.hue,
        detail: row.detail,
        note: row.right.or_else(|| raw.note.map(str::to_owned)),
        hidden: hidden.filter(|_| !open),
        time: raw.ms.and_then(elapsed),
        view: row.view,
        tail: if open {
            Vec::new()
        } else {
            tail.iter().map(|row| line(row, LineClass::Dim)).collect()
        },
        open,
        action: (!body.is_empty() || !matches!(rest, Rest::Whole)).then(|| rest.toggle(key, cx)),
        body: if open { body } else { Vec::new() },
        more: rest.more(cx),
        fold: Fold {
            key: key.to_owned(),
            tool: raw.tool.to_owned(),
            facet: row.facet,
            ms: raw.ms.unwrap_or(0),
        },
    }
}

/// JSON text as a value; `Null` when absent or clipped.
pub(super) fn parse(json: &str) -> Value {
    serde_json::from_str(json).unwrap_or(Value::Null)
}

/// A recorded tool call, or the half of one this page holds: a tool row, a Code Mode cell or a
/// build card.
pub(super) fn call(
    item: &SessionItem,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Option<MessageItem> {
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
        diff,
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
    let details = result
        .as_deref()
        .map(parse)
        .and_then(|value| value.get("details").cloned())
        .unwrap_or(Value::Null);
    let output = output.as_deref().unwrap_or_default();
    let shown = Call {
        name,
        args: parse(arguments),
        details,
        text: output,
        state: match state {
            SessionToolState::Running => CallState::Running,
            SessionToolState::Done => CallState::Done,
            SessionToolState::Failed => CallState::Failed,
            SessionToolState::NotRun => CallState::NotRun,
        },
        session: cx.id,
        journal: cx.journal,
        lines: None,
    };
    match name.as_str() {
        "codemode" => {
            return Some(MessageItem::Cell(code_cell::cell(
                &shown,
                call_id,
                &key,
                *elapsed_ms,
                rest,
                cx,
            )));
        }
        "task" | "build_finish" | "board_plan" => {
            let partial = !matches!(rest, Rest::Whole);
            return Some(MessageItem::Build(build_cards::card(
                &shown, call_id, partial, cx,
            )));
        }
        _ => {}
    }
    let note = if *paired {
        None
    } else if arguments.trim().is_empty() {
        Some("its call is not on this page")
    } else if result.is_none() {
        Some("no result recorded yet")
    } else {
        None
    };
    let row = present(&shown);
    Some(MessageItem::Tool(finish(
        row,
        &key,
        Raw {
            tool: name,
            summary,
            arguments,
            output,
            call: call.as_deref(),
            result: result.as_deref(),
            diff: diff.as_deref(),
            content: content.as_deref(),
            ms: *elapsed_ms,
            note,
        },
        rest,
        cx,
    )))
}

/// A call of the live turn, in its state, with the tail of its output so far.
pub(super) fn live(tool: &LiveTool, cx: Cx<'_>) -> MessageItem {
    // The same key as the recorded call, so an opened row stays open once it is recorded.
    let key = format!("call:{}", tool.call);
    let arguments = tool.args.as_deref().unwrap_or_default();
    let partial = tool.partial.as_deref().unwrap_or_default();
    let shown = Call {
        name: &tool.tool,
        args: parse(arguments),
        details: Value::Null,
        text: partial,
        state: match tool.state {
            LiveToolState::Running if tool.args_truncated => CallState::Writing,
            LiveToolState::Running => CallState::Running,
            LiveToolState::Done => CallState::Done,
            LiveToolState::Failed => CallState::Failed,
        },
        session: cx.id,
        journal: cx.journal,
        lines: None,
    };
    match tool.tool.as_str() {
        "codemode" => {
            return MessageItem::Cell(code_cell::cell(
                &shown,
                &tool.call,
                &key,
                tool.elapsed_ms,
                Rest::Whole,
                cx,
            ));
        }
        "task" | "build_finish" | "board_plan" => {
            return MessageItem::Build(build_cards::card(&shown, &tool.call, false, cx));
        }
        _ => {}
    }
    let row = present(&shown);
    MessageItem::Tool(finish(
        row,
        &key,
        Raw {
            tool: &tool.tool,
            summary: "",
            arguments,
            output: partial,
            call: None,
            result: None,
            diff: None,
            content: None,
            ms: tool.elapsed_ms,
            note: tool
                .args_truncated
                .then_some("arguments cut at the stream's bound"),
        },
        Rest::Whole,
        cx,
    ))
}

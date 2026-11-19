//! Rows for commands, as `tool-lines-jobs.ts` words them: `bash` (managed and Pi's own) and
//! `job_start` name the command's first line (the session's own `cd` dropped, a heredoc
//! summarised); a success is one row with its exit code, a failure adds its last 3 output lines.
//! A command that outlived its call stays `running in the background` with its job id and the
//! Agents view until the job's recorded result settles it. `job_wait`, `job_output`, `job_kill`
//! and `corral_job_monitor` read `wait`, `output`, `stop` and `watching`, the command beside the
//! job id. The owner's own `!command` is a `bash` row marked `you`. An absent exit code is never
//! read as 0.

use corral_protocol::detail::SessionItem;
use serde_json::Value;

use crate::model::hue::Hue;

use super::marks::short_id;
use super::replay::{Cx, Rest};
use super::tool::{
    Call, CallState, Raw, Row, ToolState, View, finish, head, last_rows, plural, rows_of, str_of,
    text,
};
use super::{MessageItem, Tool};

/// Output rows a failed command keeps, and a running one's tail.
const FAILED_ROWS: usize = 3;
const RUNNING_ROWS: usize = 2;

pub(super) fn present(call: &Call<'_>) -> Option<Row> {
    Some(match call.name {
        "bash" | "job_start" => bash(call),
        "job_wait" => wait(call),
        "job_output" => output(call),
        "job_kill" => kill(call),
        "corral_job_monitor" => monitor(call),
        _ => return None,
    })
}

/// The command as one line: a leading `cd <session dir> &&` dropped, a heredoc summarised,
/// `(+n lines)` for more.
pub(super) fn command_line(command: &str, call: &Call<'_>) -> String {
    let mut text = command.trim_start().to_owned();
    if let Some(rest) = text.strip_prefix("cd ") {
        let rest = rest.trim_start();
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ';' || c == '&')
            .unwrap_or(rest.len());
        let place = rest[..end].trim_matches(|c| c == '"' || c == '\'');
        let after = rest[end..].trim_start();
        let after = after
            .strip_prefix("&&")
            .or_else(|| after.strip_prefix(';'))
            .map(str::trim_start);
        if let Some(after) = after {
            let place = call.path(place);
            text = if place == "." {
                after.to_owned()
            } else {
                format!("cd {place} && {after}")
            };
        }
    }
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.first().map_or("", |line| line.trim_end());
    if let Some(at) = first.find("<<") {
        let marker: String = first[at + 2..]
            .trim_start_matches('-')
            .trim_start()
            .trim_matches(|c| c == '\'' || c == '"')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !marker.is_empty() {
            let end = lines
                .iter()
                .skip(1)
                .position(|line| line.trim() == marker)
                .map(|at| at + 1);
            let body = end.map_or(lines.len().saturating_sub(1), |end| end - 1);
            let after = end.map_or(0, |end| {
                lines[end + 1..]
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .count()
            });
            let more = if after > 0 {
                format!(", +{after} more lines")
            } else {
                String::new()
            };
            return format!("{first} … {marker} ({body}-line heredoc{more})");
        }
    }
    let more = lines
        .iter()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .count();
    if more > 0 {
        format!("{first} (+{more} lines)")
    } else {
        first.to_owned()
    }
}

/// How a command ended, in words; `ok` only for exit 0 without a signal. A result without an
/// exit code reads `exited`, never `exit 0`.
struct Exit {
    ok: bool,
    words: String,
}

fn exit_of(result: &Value) -> Exit {
    let signal = str_of(result, "signal");
    match str_of(result, "termination") {
        Some("deadline") => {
            return Exit {
                ok: false,
                words: "timed out".into(),
            };
        }
        Some("cancelled") => {
            return Exit {
                ok: false,
                words: signal
                    .map_or_else(|| "stopped".into(), |signal| format!("stopped ({signal})")),
            };
        }
        Some("host_stopped") => {
            return Exit {
                ok: false,
                words: "stopped with the host".into(),
            };
        }
        _ => {}
    }
    if let Some(signal) = signal {
        return Exit {
            ok: false,
            words: format!("killed by {signal}"),
        };
    }
    match result.get("exit_code").and_then(Value::as_i64) {
        Some(code) => Exit {
            ok: code == 0,
            words: format!("exit {code}"),
        },
        None => Exit {
            ok: true,
            words: "exited".into(),
        },
    }
}

/// Pi's own bash appends its status to the output: split it off.
fn sdk_output(text: &str) -> (String, Option<Exit>) {
    let mut lines: Vec<&str> = text.trim_end().lines().collect();
    let mut exit = None;
    while let Some(last) = lines.last().map(|line| line.trim()) {
        if let Some(code) = last.strip_prefix("Command exited with code ") {
            exit = Some(Exit {
                ok: code == "0",
                words: format!("exit {code}"),
            });
        } else if let Some(rest) = last.strip_prefix("Command timed out after ") {
            let seconds = rest.split(' ').next().unwrap_or_default();
            exit = Some(Exit {
                ok: false,
                words: format!("timed out after {seconds}s"),
            });
        } else if last == "Command aborted" {
            exit = Some(Exit {
                ok: false,
                words: "stopped".into(),
            });
        } else if !(last.is_empty()
            || (last.starts_with('[')
                && last.ends_with(']')
                && ["[Full output", "[Truncated", "[Showing"]
                    .iter()
                    .any(|lead| last.starts_with(lead))))
        {
            break;
        }
        lines.pop();
    }
    let output = lines.join("\n");
    let output = if output.trim() == "(no output)" {
        String::new()
    } else {
        output
    };
    (output, exit)
}

/// The last line worth reading after a success: three letters or digits, never a `set -x`
/// trace nor a line of JSON.
fn last_words(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .find(|line| {
            let line = line.trim_start();
            !line.starts_with("+ ")
                && !line.starts_with('{')
                && !line.starts_with('[')
                && line.chars().filter(|c| c.is_alphanumeric()).count() >= 3
        })
        .map(|line| line.trim().to_owned())
}

/// A finished command: `✓ exit 0` (`exit 0 · no output`), or `✗ exit 1` with its last 3 lines.
fn ended(row: Row, exit: Exit, output: &str, failed: bool) -> Row {
    let count = rows_of(output).len() as u64;
    if exit.ok && !failed {
        let mut row = row.ok(if count == 0 {
            format!("{} · no output", exit.words)
        } else {
            exit.words
        });
        row.detail = last_words(output);
        row.hidden = (count > 0).then(|| plural(count, "line", "lines"));
        return row;
    }
    let mut row = row.fail(if exit.ok { "failed".into() } else { exit.words });
    row.body = last_rows(output, FAILED_ROWS);
    row.hidden = (count > FAILED_ROWS as u64).then(|| format!("{} more lines", count - 3));
    row
}

/// `cargo test (j1-4abcd)`: the command the page holds for the job, its id shortened (the
/// whole id is in the opened row), or the short id alone.
fn job_target(id: &str, call: &Call<'_>) -> String {
    match call.journal.commands.get(id) {
        Some(command) => format!("{} ({})", command_line(command, call), short_id(id)),
        None => format!("job ({})", short_id(id)),
    }
}

/// The output inside a job result: its `text` field, else the text after its metadata.
fn job_output(details: &Value, text: &str) -> String {
    if let Some(own) = details.get("text").and_then(Value::as_str) {
        return own.to_owned();
    }
    const MARK: &str = "Output (untrusted data):\n";
    match text.find(MARK) {
        Some(at) => text[at + MARK.len()..].to_owned(),
        None => text.to_owned(),
    }
}

fn failed_with(row: Row, text: &str, details: &Value) -> Row {
    match str_of(details, "error") {
        Some(error) => row.fail(error),
        None => row.failed_with(text),
    }
}

fn bash(call: &Call<'_>) -> Row {
    let command = call.arg("command").unwrap_or_default();
    let mut row = Row::new(call, "bash").target(command_line(command, call));
    let details = &call.details;
    let job = str_of(details, "job");
    let output = if job.is_some() {
        job_output(details, call.text)
    } else {
        sdk_output(call.text).0
    };
    row.expanded.extend(text("command", command));
    row.expanded.extend(text("output", &output));
    match call.state {
        CallState::Writing | CallState::NotRun => return row,
        CallState::Running => {
            row.right = call
                .args
                .get("timeout")
                .and_then(Value::as_u64)
                .and_then(|seconds| super::tool::elapsed(seconds * 1000))
                .map(|time| format!("timeout {time}"));
            row.body = last_rows(call.text, RUNNING_ROWS);
            return row;
        }
        CallState::Done | CallState::Failed => {}
    }
    let failed = call.state == CallState::Failed;
    if let Some(job) = job {
        return match str_of(details, "state") {
            Some("running") => background(row, job, call),
            Some("exited") => ended(row, exit_of(details), &output, failed),
            _ if str_of(details, "error").is_some() => failed_with(row, call.text, details),
            _ => ended(row, exit_of(details), &output, failed),
        };
    }
    if str_of(details, "error").is_some() {
        return failed_with(row, call.text, details);
    }
    let (output, exit) = sdk_output(call.text);
    match exit {
        Some(exit) => ended(row, exit, &output, failed),
        // A blocked or refused call: its text is the reason.
        None if failed => row.failed_with(call.text),
        None => ended(
            row,
            Exit {
                ok: true,
                words: "exit 0".into(),
            },
            &output,
            false,
        ),
    }
}

/// A command still running after its call returned: settled by the job's recorded result,
/// running while the session's host runs, else continued where the page cannot follow it.
fn background(mut row: Row, job: &str, call: &Call<'_>) -> Row {
    row.right = Some(short_id(job));
    row.expanded.extend(text("job", job));
    row.view = Some(call.view(View::Agents));
    match call.journal.jobs.get(job) {
        Some(result) => {
            let output = job_output(result, "");
            ended(row, exit_of(result), &output, false)
        }
        None if call.journal.live => row.state(ToolState::Running, "running in the background"),
        None => row.state(ToolState::Waiting, "continued in the background"),
    }
}

fn wait(call: &Call<'_>) -> Row {
    let details = &call.details;
    let id = call.arg("job").or_else(|| str_of(details, "job"));
    let output = job_output(details, call.text);
    let mut row = Row::new(call, "wait").target(match id {
        Some(id) => job_target(id, call),
        None => "next job to finish".to_owned(),
    });
    if let Some(id) = id {
        row.view = Some(call.view(View::Agents));
        row.expanded.extend(text("job", id));
    }
    row.expanded.extend(text("output", &output));
    match call.state {
        CallState::Writing | CallState::NotRun => return row,
        CallState::Running => {
            row.right = call
                .args
                .get("wait_ms")
                .and_then(Value::as_u64)
                .and_then(super::tool::elapsed)
                .map(|time| format!("up to {time}"));
            return row;
        }
        CallState::Done | CallState::Failed => {}
    }
    match str_of(details, "state") {
        Some("exited") => ended(row, exit_of(details), &output, false),
        Some("running") => row.state(ToolState::Waiting, "still running · stopped waiting"),
        Some("waiting") => row.state(ToolState::Waiting, "nothing finished yet"),
        Some("none") => row.state(ToolState::Waiting, "no jobs to wait for"),
        Some("wait_cancelled") => {
            row.state(ToolState::Waiting, "wait cancelled · the job keeps running")
        }
        _ if call.state == CallState::Failed => failed_with(row, call.text, details),
        _ => {
            let first = head(call.text).to_owned();
            row.says(first)
        }
    }
}

fn output(call: &Call<'_>) -> Row {
    let details = &call.details;
    let id = call.arg("job").unwrap_or_default();
    let shown = job_output(details, call.text);
    let count = call.lines.unwrap_or(rows_of(&shown).len() as u64);
    let mut row = Row::new(call, "output");
    if !id.is_empty() {
        row = row.target(job_target(id, call));
        row.expanded.extend(text("job", id));
        row.view = Some(call.view(View::Agents));
    }
    row.expanded.extend(text("output", &shown));
    row.hidden = (count > 0).then(|| plural(count, "line", "lines"));
    match call.state {
        CallState::Done => {}
        CallState::Failed => return failed_with(row, call.text, details),
        _ => return row,
    }
    if str_of(details, "error").is_some() {
        return failed_with(row, call.text, details);
    }
    let exit = (str_of(details, "state") == Some("exited")).then(|| exit_of(details));
    let failed = exit.as_ref().is_some_and(|exit| !exit.ok);
    let mut words = vec![exit.map_or_else(|| "running".to_owned(), |exit| exit.words)];
    if count == 0 {
        words.push("no output".into());
    }
    if details.get("retained_snapshot").and_then(Value::as_bool) == Some(true) {
        words.push("retained snapshot".into());
    }
    let mut row = row.ok(words.join(" · "));
    if failed {
        row.hue = Hue::Fail;
    }
    row.detail = last_words(&shown);
    row
}

fn kill(call: &Call<'_>) -> Row {
    let details = &call.details;
    let id = call.arg("job").unwrap_or_default();
    let mut row = Row::new(call, "stop");
    if !id.is_empty() {
        row = row.target(job_target(id, call));
        row.expanded.extend(text("job", id));
    }
    match call.state {
        CallState::Done => {}
        CallState::Failed => return failed_with(row, call.text, details),
        _ => return row,
    }
    if str_of(details, "error").is_some() {
        return failed_with(row, call.text, details);
    }
    if str_of(details, "state") == Some("lost") {
        return row.state(ToolState::Unknown, "job no longer tracked");
    }
    let before = if str_of(details, "termination") == Some("exit") {
        "had already exited".to_owned()
    } else {
        match str_of(details, "signal") {
            Some(signal) => format!("stopped ({signal})"),
            None => "stopped".to_owned(),
        }
    };
    row.ok(format!("{before} · output kept"))
}

fn monitor(call: &Call<'_>) -> Row {
    let details = &call.details;
    let id = call.arg("job").unwrap_or_default();
    let wake = match call.arg("on") {
        Some("line") => format!(
            "until /{}/ matches",
            call.arg("pattern").unwrap_or_default()
        ),
        _ => "until it exits".to_owned(),
    };
    let mut row = Row::new(call, "watching");
    if !id.is_empty() {
        row = row.target(job_target(id, call));
        row.expanded.extend(text("job", id));
    }
    match call.state {
        CallState::Done => {}
        CallState::Failed => return failed_with(row, call.text, details),
        _ => return row,
    }
    if str_of(details, "error").is_some() {
        return failed_with(row, call.text, details);
    }
    let result = details.get("result").unwrap_or(&Value::Null);
    match str_of(details, "state") {
        Some("rejected") => {
            let reason: Vec<&str> = [
                str_of(result, "code"),
                str_of(result, "reason").or_else(|| str_of(result, "message")),
            ]
            .into_iter()
            .flatten()
            .collect();
            row.fail(if reason.is_empty() {
                "refused".to_owned()
            } else {
                reason.join(": ")
            })
        }
        Some("committed") if result.get("settled").and_then(Value::as_bool) == Some(true) => {
            row.verb = "watched".into();
            row.ok("already settled")
        }
        Some("committed") => {
            let until = str_of(result, "deadline")
                .and_then(|deadline| deadline.get(11..16))
                .map(|clock| format!(" · until {clock}"))
                .unwrap_or_default();
            row.state(ToolState::Waiting, format!("{wake}{until}"))
        }
        other => row.state(
            ToolState::Unknown,
            format!("{wake} · {}", other.unwrap_or("not confirmed")),
        ),
    }
}

/// The owner's own `!command` (`!!` keeps its output from the agent): a row in the bash
/// grammar marked as the owner's, with its exit, cancelled or cut.
pub(super) fn owner(
    item: &SessionItem,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Option<MessageItem> {
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
    let call = Call {
        name: "bash",
        args: Value::Null,
        details: Value::Null,
        text: output,
        state: CallState::Done,
        session: cx.id,
        journal: cx.journal,
        lines: None,
    };
    let row = Row::new(&call, "bash").target(command_line(command, &call));
    let mut row = if *cancelled {
        let mut row = row.fail("cancelled");
        row.body = last_rows(output, FAILED_ROWS);
        row
    } else {
        match exit_code {
            Some(code) => ended(
                row,
                Exit {
                    ok: *code == 0,
                    words: format!("exit {code}"),
                },
                output,
                false,
            ),
            None => row.state(ToolState::Unknown, "exit unknown"),
        }
    };
    if *truncated {
        row.outcome.push_str(" · output cut");
    }
    row.right = Some(if *exclude_from_context {
        "you · not sent to the agent".to_owned()
    } else {
        "you".to_owned()
    });
    row.expanded.extend(text("command", command));
    row.expanded.extend(text("output", output));
    row.expanded.extend(
        full_output_path
            .as_deref()
            .and_then(|path| text("full output saved at", path)),
    );
    let tool: Tool = finish(
        row,
        key,
        Raw {
            tool: "bash",
            summary: "",
            arguments: "",
            output,
            call: None,
            result: None,
            diff: None,
            content: None,
            ms: None,
            note: None,
        },
        rest,
        cx,
    );
    Some(MessageItem::Tool(tool))
}

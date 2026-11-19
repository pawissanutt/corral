//! Rows for corral operations and owner questions, as `corral-presenter.ts` words them: the
//! verb speaks as the agent (`Finished the run · sent to review`, `Requested changes`,
//! `Attached report: Session record`, `Saved checkpoint`), the outcome comes from the structured
//! result or its refusal `{reason, next}`, never from the text the model read, and no request id
//! shows. A failure takes the operation's noun with `refused`, `not sent` or `failed`, its reason
//! as a row and, opened, each `next:` step. `ask` reads `▲ waiting for you`, `answered: <choice>`
//! or why there was no answer.

use serde_json::Value;

use super::Section;
use super::tool::{Call, CallState, Row, ToolState, View, head, list_of, plural, str_of};

/// Prose rows kept under a settled row.
const PROSE_ROWS: usize = 2;
const REASON_ROWS: usize = 3;

pub(super) fn present(call: &Call<'_>) -> Option<Row> {
    match call.name {
        "corral_done" | "corral_verdict" | "corral_artifact" | "corral_todo"
        | "corral_checkpoint" => Some(operation(call)),
        "ask_user" => Some(ask_user(call)),
        "ask_owner" => Some(ask_owner(call)),
        _ => None,
    }
}

/// How a corral call settled.
enum Settled {
    Result(Value),
    Failure { reason: String, next: Vec<String> },
    Schema(String),
    Text(String),
}

fn next_lines(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| match step {
                    Value::String(text) if !text.trim().is_empty() => {
                        Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
                    }
                    Value::String(_) => None,
                    other => Some(other.to_string()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn settle(call: &Call<'_>) -> Option<Settled> {
    if !matches!(call.state, CallState::Done | CallState::Failed) {
        return None;
    }
    let text = call.text;
    if text.starts_with("Validation failed for tool") {
        let field = text
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix("- "))
            .unwrap_or("the arguments do not match the tool");
        return Some(Settled::Schema(field.to_owned()));
    }
    let parsed: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    if call.state == CallState::Failed {
        let failure = if str_of(&call.details, "reason").is_some() {
            &call.details
        } else {
            &parsed
        };
        if let Some(reason) = str_of(failure, "reason") {
            return Some(Settled::Failure {
                reason: reason.to_owned(),
                next: next_lines(failure.get("next")),
            });
        }
        let mut lines = text.trim().lines();
        if let Some(reason) = lines
            .next()
            .and_then(|first| first.strip_prefix("refused: "))
        {
            let rest: Vec<&str> = lines.collect();
            let at = rest.iter().position(|line| line.starts_with("next: "));
            let mut words = vec![reason];
            words.extend(rest.iter().take(at.unwrap_or(rest.len())));
            let next = rest
                .iter()
                .filter_map(|line| line.strip_prefix("next: "))
                .map(str::to_owned)
                .collect();
            return Some(Settled::Failure {
                reason: words.join("\n"),
                next,
            });
        }
        return Some(Settled::Text(text.to_owned()));
    }
    Some(Settled::Result(match &call.details {
        Value::Object(fields) if !fields.is_empty() => call.details.clone(),
        _ => parsed,
    }))
}

/// A reason with known codes said in words, and a bare code with its underscores as spaces.
fn plain(raw: &str) -> String {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_end_matches(['.', ';', ' ']).to_owned();
    let (code, rest) = match text.split_once(':') {
        Some((code, rest)) => (code, Some(rest.trim())),
        None => (text.as_str(), None),
    };
    let coded = code.contains('_')
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !coded {
        return text;
    }
    let known = match code {
        "multiple_current_steps" => Some("another step is already in progress"),
        "invalid_todo_wait" => Some("that wait is not valid for this step"),
        "terminal_todo_step" => Some("that step is already finished"),
        _ => None,
    };
    match (known, rest) {
        (Some(known), _) => known.to_owned(),
        (None, Some(rest)) if !rest.is_empty() => format!("{}: {rest}", code.replace('_', " ")),
        _ => code.replace('_', " "),
    }
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The plan step a todo call names, from its result's plan when it has one.
fn step_label(result: Option<&Value>, step: &str) -> String {
    result
        .and_then(|result| result.get("progress"))
        .map(|progress| list_of(progress, "phases"))
        .into_iter()
        .flatten()
        .flat_map(|phase| list_of(phase, "steps"))
        .find(|item| str_of(item, "id") == Some(step))
        .and_then(|item| str_of(item, "label"))
        .unwrap_or(step)
        .to_owned()
}

/// `plan 5/7`: done steps of the steps not dropped.
fn position(result: &Value) -> Option<String> {
    let progress = result.get("progress")?;
    let states: Vec<&str> = list_of(progress, "phases")
        .iter()
        .flat_map(|phase| list_of(phase, "steps"))
        .map(|step| {
            step.get("status")
                .and_then(|status| str_of(status, "state"))
                .unwrap_or("pending")
        })
        .filter(|state| *state != "dropped")
        .collect();
    let done = states.iter().filter(|state| **state == "done").count();
    Some(format!("plan {done}/{}", states.len()))
}

fn operation(call: &Call<'_>) -> Row {
    let args = &call.args;
    let settled = settle(call);
    let result = match &settled {
        Some(Settled::Result(result)) => Some(result),
        _ => None,
    };
    let op = call.name.trim_start_matches("corral_");
    let todo = str_of(args, "op").unwrap_or_default();
    let (did, doing, noun) = match op {
        "done" => ("Finished the run", "Finishing the run", "Finish"),
        "verdict" => (
            match str_of(args, "verdict") {
                Some("approve") => "Approved the candidate",
                Some("changes") => "Requested changes",
                _ => "Recorded the verdict",
            },
            "Recording the verdict",
            "Verdict",
        ),
        "artifact" => ("Attached", "Attaching", "Attach"),
        "todo" => (
            match todo {
                "init" => "Opened a plan",
                "append" => "Added steps to",
                "start" => "Started step",
                "done" => "Finished step",
                "drop" => "Dropped step",
                "wait" => "Step waits",
                _ => "Updated the plan",
            },
            match todo {
                "init" => "Opening a plan",
                "append" => "Adding steps to",
                "start" => "Starting step",
                "done" => "Finishing step",
                "drop" => "Dropping step",
                "wait" => "Parking step",
                _ => "Updating the plan",
            },
            "Plan update",
        ),
        _ => ("Saved checkpoint", "Saving a checkpoint", "Checkpoint"),
    };
    let known = !matches!(settled, Some(Settled::Schema(_)));
    let target = if !known {
        None
    } else {
        match op {
            "artifact" => str_of(args, "title")
                .map(|title| match str_of(args, "kind") {
                    Some(kind) => format!("{kind}: {}", flat(title)),
                    None => flat(title),
                })
                .or_else(|| {
                    str_of(args, "path")
                        .map(|path| path.rsplit(['/', ':']).next().unwrap_or(path).to_owned())
                }),
            "todo" => match todo {
                "init" => result
                    .and_then(|result| str_of(result, "plan"))
                    .map(str::to_owned),
                "append" => str_of(args, "label")
                    .or_else(|| str_of(args, "phase"))
                    .map(str::to_owned),
                _ => str_of(args, "step").map(|step| step_label(result, step)),
            },
            _ => None,
        }
    };
    let prose = if !known {
        None
    } else {
        match op {
            "done" => str_of(args, "summary"),
            "verdict" => str_of(args, "notes"),
            "checkpoint" => str_of(args, "note"),
            "todo" if todo == "drop" => str_of(args, "reason"),
            "todo" => args.get("on").and_then(|on| str_of(on, "question")),
            _ => None,
        }
    };
    let view = match op {
        "done" => Some(call.view(View::Task)),
        "verdict" => Some(call.view(View::Proof)),
        _ => None,
    };
    let verb = match &settled {
        None => doing,
        Some(Settled::Result(_)) => did,
        Some(_) => noun,
    };
    let mut row = Row::new(call, verb).target(target.unwrap_or_default());
    row.view = view;
    row.facet = match op {
        "todo" => Some(todo.to_owned()).filter(|op| !op.is_empty()),
        "artifact" => str_of(args, "kind").map(str::to_owned),
        _ => None,
    };
    if let Some(prose) = prose {
        row.expanded.push(Section::Prose(prose.to_owned()));
    }
    match settled {
        None => {
            if let Some(prose) = prose {
                row.body = prose.lines().rev().take(3).map(str::to_owned).collect();
                row.body.reverse();
            }
            row
        }
        Some(Settled::Result(result)) => {
            if let Some(prose) = prose {
                row.body = prose.lines().take(PROSE_ROWS).map(str::to_owned).collect();
            }
            match op {
                "done" => row.ok("sent to review"),
                "todo" if todo == "init" => {
                    let phases = result
                        .get("progress")
                        .map_or(0, |progress| list_of(progress, "phases").len());
                    let steps: usize = result
                        .get("progress")
                        .map(|progress| {
                            list_of(progress, "phases")
                                .iter()
                                .map(|phase| list_of(phase, "steps").len())
                                .sum()
                        })
                        .unwrap_or(0);
                    row.ok(format!(
                        "{}, {}",
                        plural(phases as u64, "phase", "phases"),
                        plural(steps as u64, "step", "steps")
                    ))
                }
                "todo" if todo == "append" => {
                    let added = plural(list_of(args, "steps").len() as u64, "step", "steps");
                    let outcome = match position(&result) {
                        Some(position) => format!("+{added} · {position}"),
                        None => format!("+{added}"),
                    };
                    row.ok(outcome)
                }
                "todo" if todo == "wait" => {
                    let on = args.get("on").unwrap_or(&Value::Null);
                    let owner = str_of(on, "kind") == Some("owner")
                        || result
                            .get("disposition")
                            .and_then(|disposition| str_of(disposition, "state"))
                            == Some("awaiting_owner");
                    if owner {
                        row.state(ToolState::NeedsYou, "waiting for your answer")
                    } else {
                        row.state(
                            ToolState::Waiting,
                            str_of(on, "event").map_or_else(
                                || "waiting".to_owned(),
                                |event| format!("on {}", flat(event)),
                            ),
                        )
                    }
                }
                "todo" => match position(&result) {
                    Some(position) => row.ok(position),
                    None => row.ok(""),
                },
                _ => row.ok(""),
            }
        }
        Some(Settled::Schema(field)) => {
            let mut row = row.fail("not sent");
            row.body = vec![field];
            row
        }
        Some(Settled::Text(text)) => {
            let reason = head(&text).to_owned();
            let mut row = row.fail("failed");
            row.body = vec![reason];
            row
        }
        Some(Settled::Failure { reason, next }) => {
            let (word, reason) = if reason.starts_with("Invalid arguments:") {
                ("not sent", reason)
            } else {
                ("refused", plain(&reason))
            };
            let mut row = row.fail(word);
            row.body = reason
                .lines()
                .take(REASON_ROWS)
                .map(str::to_owned)
                .collect();
            row.expanded.push(Section::Text {
                label: "reason".into(),
                text: reason.clone(),
            });
            if !next.is_empty() {
                row.expanded.push(Section::Text {
                    label: "next".into(),
                    text: next
                        .iter()
                        .map(|step| format!("next: {step}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            }
            row
        }
    }
}

/// `ask <first question> + n more`.
fn question_target(args: &Value) -> Option<String> {
    let questions = list_of(args, "questions");
    let first = questions.first()?;
    let label = str_of(first, "header")
        .or_else(|| str_of(first, "question"))
        .unwrap_or_default();
    Some(match questions.len() {
        0 | 1 => label.to_owned(),
        n => format!("{label} + {} more", n - 1),
    })
}

/// What one answer chose: its option, else its text.
fn answer_of(answer: &Value) -> Option<String> {
    str_of(answer, "option")
        .or_else(|| str_of(answer, "text"))
        .or_else(|| answer.as_str())
        .map(|text| head(text).to_owned())
}

fn ask_user(call: &Call<'_>) -> Row {
    let withdraw = call.arg("withdraw");
    let mut row = Row::new(call, "ask");
    if let Some(id) = withdraw {
        row = row.target(format!("withdraw {id}"));
    } else if let Some(target) = question_target(&call.args) {
        row = row.target(target);
    }
    let details = &call.details;
    match call.state {
        CallState::Writing => return row,
        CallState::Running if withdraw.is_some() => return row,
        CallState::Running => return row.state(ToolState::NeedsYou, "waiting for you"),
        CallState::Failed => return row.failed_with(call.text),
        CallState::NotRun => return row,
        CallState::Done => {}
    }
    let status = str_of(details, "status");
    if withdraw.is_some() {
        let outcome = status.unwrap_or_else(|| head(call.text)).to_owned();
        return row.ok(outcome);
    }
    let answers = list_of(details, "answers");
    if !answers.is_empty() {
        let unanswered = answers
            .iter()
            .filter(|answer| answer_of(answer).is_none())
            .count();
        if answers.len() == 1
            && let Some(choice) = answer_of(&answers[0])
        {
            return row.ok(format!("answered: {choice}"));
        }
        let mut outcome = format!("{} answered", answers.len() - unanswered);
        if unanswered > 0 {
            outcome.push_str(&format!(" · {unanswered} unanswered"));
        }
        return row.ok(outcome);
    }
    match status {
        Some("pending") => row.state(ToolState::NeedsYou, "waiting for you"),
        Some(status) if status != "answered" => {
            let reason = str_of(details, "reason");
            let words = match reason {
                Some("another owner/permission dialog is open") => {
                    "no answer: another dialog was open".to_owned()
                }
                Some("pending UI unavailable") => {
                    "no answer: no interactive pane to ask in".to_owned()
                }
                Some(reason) => format!("no answer: {reason}"),
                None => "no answer: dismissed or timed out".to_owned(),
            };
            row.state(ToolState::Waiting, words)
        }
        _ => {
            let first = head(call.text).to_owned();
            row.ok(first)
        }
    }
}

fn ask_owner(call: &Call<'_>) -> Row {
    let options = list_of(&call.args, "options").len() as u64;
    let mut row = Row::new(call, "ask").target(call.arg("question").unwrap_or_default());
    if options > 0 {
        row.right = Some(plural(options, "option", "options"));
    }
    match call.state {
        CallState::Running => row.state(ToolState::NeedsYou, "waiting for you"),
        CallState::Failed => row.failed_with(call.text),
        CallState::Done if !call.text.trim().is_empty() => {
            let first = head(call.text).to_owned();
            row.ok(format!("answered: {first}"))
        }
        CallState::Done => row.ok("no answer"),
        CallState::Writing | CallState::NotRun => row,
    }
}

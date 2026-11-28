//! What the page's own journal entries say about rows elsewhere on it. The host records a
//! background job's completion (`corral_command`) and a detached Code Mode cell's settlement
//! (`corral_code_cell`) as entries of their own; the terminal UI draws none of them but settles
//! the command's row and the cell's row from them. The app does the same, and learns here which
//! `build_finish` card is the latest and whether the host recorded its own model marks.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{SessionItem, SessionMessage};
use serde_json::Value;

use crate::model::Id;
use crate::model::daemon::Feed;

use super::pairs::Pairs;
use super::replay::{Rest, resolve};

/// Entries whose facts settle another row; they are never rows themselves.
pub(super) const FACT_ENTRIES: [&str; 4] = [
    "corral_command",
    "corral_command_monitor",
    "corral_job_control",
    "corral_code_call",
];

#[derive(Debug, Default)]
pub(super) struct Journal {
    /// Each background job's latest recorded result, by job id.
    pub jobs: BTreeMap<String, Value>,
    /// The command each job of the page runs, by job id: job tools name it beside the id.
    pub commands: BTreeMap<String, String>,
    /// Each owner question's first question, by its id: rows name a question by its words.
    pub questions: BTreeMap<String, String>,
    /// Each detached Code Mode cell's settlement, by the call that started the cell.
    pub cells: BTreeMap<String, Value>,
    /// The Code Mode calls on this page.
    pub codemode: BTreeSet<String>,
    /// The page's latest `build_finish` call: only its card follows the exit.
    pub finish: Option<String>,
    /// The host recorded its own model marks, which say what Pi's model and effort entries say.
    pub model_marks: bool,
    /// The session's working directory, when the session row names it: paths read relative to it.
    pub cwd: Option<String>,
    /// The session's host streams its state: a job not settled yet still runs.
    pub live: bool,
}

impl Journal {
    pub fn of(id: &Id, messages: &[SessionMessage], pairs: &Pairs<'_>, feed: &Feed) -> Journal {
        let mut journal = Journal {
            cwd: feed
                .assistants
                .get(id.as_str())
                .and_then(|row| row.build.as_ref())
                .map(|build| build.worktree.clone()),
            live: feed
                .live
                .get(id.as_str())
                .is_some_and(|live| live.state.is_some()),
            ..Journal::default()
        };
        for message in messages {
            for (item, rest) in resolve(&message.items, feed) {
                // Job labels need both the command from the call and the id from its result.
                if let SessionItem::ToolCall { call_id, .. } = item
                    && let Some(pair) = pairs.get(call_id.as_str())
                {
                    if std::ptr::eq(item, pair.position) {
                        journal.read(pair.item.as_ref(), pair.rest);
                    }
                } else {
                    journal.read(item, rest);
                }
            }
        }
        journal
    }

    fn read(&mut self, item: &SessionItem, rest: Rest<'_>) {
        match item {
            SessionItem::ToolCall {
                call_id,
                name,
                arguments,
                result,
                ..
            } if !call_id.is_empty() => match name.as_str() {
                "eval_js" | "codemode" => {
                    self.codemode.insert(call_id.clone());
                }
                "build_finish" => self.finish = Some(call_id.clone()),
                "bash" | "job_start" => {
                    let job = result
                        .as_deref()
                        .and_then(|result| serde_json::from_str::<Value>(result).ok())
                        .and_then(|result| {
                            result
                                .pointer("/details/job")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        });
                    let command = serde_json::from_str::<Value>(arguments)
                        .ok()
                        .and_then(|args| {
                            args.get("command")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        });
                    if let (Some(job), Some(command)) = (job, command) {
                        self.commands.insert(job, command);
                    }
                }
                _ => {}
            },
            SessionItem::Card {
                title,
                data: Some(data),
                ..
            } if matches!(rest, Rest::Whole) => {
                let Ok(data) = serde_json::from_str::<Value>(data) else {
                    return;
                };
                match title.as_str() {
                    "corral_command" => {
                        if let (Some(job), Some(result)) =
                            (data.get("job").and_then(Value::as_str), data.get("result"))
                        {
                            self.jobs.insert(job.to_owned(), result.clone());
                        }
                    }
                    "corral_code_cell" => {
                        if data.get("phase").and_then(Value::as_str) == Some("settled")
                            && let Some(call) = data.get("tool_call_id").and_then(Value::as_str)
                        {
                            self.cells.insert(call.to_owned(), data.clone());
                        }
                    }
                    "corral_owner_question" => {
                        let title = data
                            .get("questions")
                            .and_then(Value::as_array)
                            .and_then(|questions| questions.first())
                            .and_then(|question| question.get("question"))
                            .and_then(Value::as_str)
                            .and_then(|question| question.lines().next());
                        if let (Some(id), Some(title)) =
                            (data.get("id").and_then(Value::as_str), title)
                        {
                            self.questions.insert(id.to_owned(), title.to_owned());
                        }
                    }
                    // A finish card the owner opened is the latest exit card too.
                    super::build_entries::FINISH_ENTRY => {
                        if let Some(id) = data.pointer("/details/id").and_then(Value::as_str) {
                            self.finish = Some(id.to_owned());
                        }
                    }
                    "corral_mark" => {
                        self.model_marks |=
                            data.get("kind").and_then(Value::as_str) == Some("model");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

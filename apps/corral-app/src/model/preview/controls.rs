//! Session controls and question answers against the sample, answered under their request id
//! as corrald answers them: a run without a pi host refuses Cancel, Pause and Rewind with
//! corrald's reason, a reviewer refuses Pause, Terminate stops a run and Relaunch retries it;
//! a sample chat changes its row's state.

use corral_protocol::rows::{AssistantLocation, AssistantState};
use corral_protocol::session::SessionPromptAnswer;
use corral_types::{Op, Refusal, Role, SessionRef, SessionStatus};
use serde_json::{Value, json};

use crate::model::Id;
use crate::model::act::Act;
use crate::model::agents::Control;
use crate::model::daemon::{ClientError, Feed, Operation};

/// Records the outcome of `control` of `session` under `key`.
pub fn control(feed: &mut Feed, key: &Id, session: &str, control: Control) {
    let outcome = if session == crate::model::assistant::QUICK || session.starts_with("A-") {
        chat(feed, session, control)
    } else {
        run(feed, key, session, control)
    };
    let mut operation = Operation {
        method: control.method().into(),
        loading: false,
        value: None,
        error: None,
        refusal: None,
    };
    match outcome {
        Ok(value) => operation.value = Some(value),
        Err(Fail::Refused(reason)) => {
            operation.refusal = Some(Refusal {
                reason,
                next: Vec::new(),
            })
        }
        Err(Fail::Missing(message)) => {
            operation.error = Some(ClientError {
                code: "not_found".into(),
                message,
            })
        }
    }
    feed.operations.insert(key.clone(), operation);
}

/// Records the outcome of an answer under `key`: the sample holds no pending question.
pub fn answer(feed: &mut Feed, key: &Id, answer: &SessionPromptAnswer) {
    feed.operations.insert(
        key.clone(),
        Operation {
            method: corral_protocol::assistant::PROMPT_ANSWER.into(),
            loading: false,
            value: Some(json!({"status": "missing"})),
            error: None,
            refusal: Some(Refusal {
                reason: format!(
                    "Preview · {} has no pending question {}",
                    answer.session, answer.question
                ),
                next: Vec::new(),
            }),
        },
    );
}

enum Fail {
    Refused(String),
    Missing(String),
}

fn run(feed: &mut Feed, key: &Id, session: &str, control: Control) -> Result<Value, Fail> {
    let row = feed
        .agents
        .get(session)
        .cloned()
        .ok_or_else(|| Fail::Missing("task session not found".into()))?;
    let task = row.run.task.clone();
    match control {
        Control::Terminate => {
            engine(feed, key, Op::Stop { id: task })?;
            Ok(Value::Null)
        }
        Control::Relaunch => {
            engine(
                feed,
                key,
                Op::Retry {
                    id: task.clone(),
                    profile: Some(row.run.profile.clone()),
                    fresh: false,
                    note: None,
                },
            )?;
            let next = feed
                .agents
                .values()
                .filter(|other| other.run.task == task && other.run.ended.is_none())
                .map(|other| format!("{}_{}", other.run.task, other.run.id))
                .last()
                .unwrap_or_else(|| session.to_owned());
            Ok(json!({"id": next}))
        }
        _ if !matches!(row.run.session, Some(SessionRef::Pi { .. })) => Err(Fail::Refused(
            "session has no Pi socket; use Terminate to stop its host".into(),
        )),
        Control::Pause if row.run.role == Role::Reviewer => Err(Fail::Refused(
            "reviewers support Cancel and Terminate, not Pause".into(),
        )),
        Control::Cancel => {
            if let Some(row) = feed.agents.get_mut(session) {
                row.status = SessionStatus::Idle;
            }
            Ok(json!({"cancelled": true}))
        }
        Control::Pause => Ok(json!({"paused": true, "cancelled": true})),
        Control::Resume => Ok(json!({"paused": false})),
        Control::Rewinds => Ok(json!({"kind": "list", "checkpoints": [], "rewinds": []})),
        Control::Rewind(_) => Err(Fail::Refused(
            "Preview · the sample host recorded no checkpoint to rewind to".into(),
        )),
    }
}

/// Submits an engine operation as corrald does for Terminate and Relaunch.
fn engine(feed: &mut Feed, key: &Id, op: Op) -> Result<(), Fail> {
    let act = Act::with_key(Id::from(format!("{key}-op")), op, false);
    super::act(feed, &act).map_err(Fail::Refused)?;
    match crate::model::act::status(feed, act.key.as_str()) {
        Some(crate::model::ActStatus::Refused(refusal)) => Err(Fail::Refused(refusal.reason)),
        Some(crate::model::ActStatus::Failed(why)) => Err(Fail::Refused(why)),
        _ => Ok(()),
    }
}

fn chat(feed: &mut Feed, session: &str, control: Control) -> Result<Value, Fail> {
    let id = if session == crate::model::assistant::QUICK {
        crate::model::assistant::quick_id(feed)
            .ok_or_else(|| Fail::Missing("chat session not found".into()))?
    } else {
        session.to_owned()
    };
    let row = feed
        .assistants
        .get_mut(id.as_str())
        .ok_or_else(|| Fail::Missing("chat session not found".into()))?;
    if row.state == AssistantState::Ended {
        return Err(Fail::Refused(
            "chat thread ended; send a message to start a new session".into(),
        ));
    }
    row.rev += 1;
    Ok(match control {
        Control::Cancel => {
            row.state = AssistantState::Ready;
            json!({"cancelled": true, "turn": row.turn})
        }
        Control::Pause => {
            row.state = AssistantState::Paused;
            json!({"paused": true, "cancelled": false})
        }
        Control::Resume => {
            row.state = AssistantState::Ready;
            json!({"paused": false})
        }
        Control::Terminate => {
            row.state = AssistantState::Cold;
            row.live_in = AssistantLocation::None;
            Value::Null
        }
        Control::Relaunch => {
            row.state = AssistantState::Ready;
            row.live_in = AssistantLocation::Daemon;
            json!({"id": id})
        }
        Control::Rewinds | Control::Rewind(_) => {
            return Err(Fail::Refused("chat sessions are not rewound".into()));
        }
    })
}

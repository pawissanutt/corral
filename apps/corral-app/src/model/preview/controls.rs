//! Session controls against the sample, answered under their request id
//! as corrald answers them: a pi host (a sample chat, or one of the two pi workers) cancels,
//! pauses, resumes and rewinds; a run without a pi host refuses those with corrald's reason, a
//! reviewer refuses Pause, and Terminate and Relaunch of a run are the engine's `stop` and
//! `retry`.

use corral_protocol::rows::AssistantState;
use corral_types::{Op, Refusal, Role, SessionRef};
use serde_json::{Value, json};

use super::{pi, pi_control};
use crate::model::Id;
use crate::model::act::Act;
use crate::model::agents::Control;
use crate::model::daemon::{ClientError, Feed, Operation};

/// Records the outcome of `control` of `session` under `key`.
pub fn control(feed: &mut Feed, key: &Id, session: &str, control: Control) {
    let hosted = session == crate::model::assistant::QUICK
        || session.starts_with("A-")
        || session.starts_with("B-");
    let outcome = if hosted {
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
                problems: None,
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

enum Fail {
    Refused(String),
    Missing(String),
}

fn run(feed: &mut Feed, key: &Id, session: &str, control: Control) -> Result<Value, Fail> {
    if control == Control::Relaunch {
        return relaunch(feed, key, session);
    }
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
        _ if !matches!(row.run.session, Some(SessionRef::Pi { .. })) => Err(Fail::Refused(
            "session has no Pi socket; use Terminate to stop its host".into(),
        )),
        Control::Pause if row.run.role == Role::Reviewer => Err(Fail::Refused(
            "reviewers support Cancel and Terminate, not Pause".into(),
        )),
        _ => pi_control::control(feed, session, control).map_err(Fail::Refused),
    }
}

/// Relaunch, as corrald does it: a retry of the run's task with the run's profile. A run that
/// ended (terminated, stopped) is no longer listed, so it is read from its task's record.
fn relaunch(feed: &mut Feed, key: &Id, session: &str) -> Result<Value, Fail> {
    let missing = || Fail::Missing("task session not found".into());
    let (task, run) = session.split_once('_').ok_or_else(missing)?;
    let profile = super::records(feed)
        .into_iter()
        .find(|record| record.task.id == task)
        .and_then(|record| record.runs.into_iter().find(|other| other.id == run))
        .map(|run| run.profile)
        .ok_or_else(missing)?;
    engine(
        feed,
        key,
        Op::Retry {
            id: task.to_owned(),
            profile: Some(profile),
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

/// A chat's controls: its host answers them; Terminate stops the host and keeps the
/// transcript, and Relaunch starts it again on the same session.
fn chat(feed: &mut Feed, session: &str, control: Control) -> Result<Value, Fail> {
    let id =
        pi::resolve(feed, session).ok_or_else(|| Fail::Missing("chat session not found".into()))?;
    let ended = feed
        .assistants
        .get(id.as_str())
        .is_none_or(|row| row.state == AssistantState::Ended);
    if ended {
        return Err(Fail::Refused(
            "chat thread ended; send a message to start a new session".into(),
        ));
    }
    match control {
        Control::Terminate => {
            pi::end(feed, &id);
            Ok(Value::Null)
        }
        Control::ResumeCode
            if !feed
                .assistants
                .get(id.as_str())
                .is_some_and(crate::model::agents::is_code) =>
        {
            Err(Fail::Refused("expected a Code session".into()))
        }
        Control::Relaunch | Control::ResumeCode => {
            pi::revive(feed, &id);
            Ok(json!({"id": id.as_str()}))
        }
        Control::Rewinds | Control::Rewind(_) => {
            Err(Fail::Refused("chat sessions are not rewound".into()))
        }
        _ => pi_control::control(feed, &id, control).map_err(Fail::Refused),
    }
}

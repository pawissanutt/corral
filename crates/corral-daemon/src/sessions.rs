//! Resolve wire session scopes to engine runs or daemon-owned chat directories.
use std::{
    fs::File,
    io::{BufReader, ErrorKind},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    Engine, Role, Run, SessionRef, refuse,
    runner::pi::{Answer, Client},
};
use corral_protocol::{
    assistant::{PromptChoice, QUICK_THREAD},
    session::*,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;

use crate::{
    assistant,
    state::Shared,
    views::{self, NotFound},
};

pub(crate) struct Session {
    pub id: String,
    pub role: &'static str,
    pub task: Option<String>,
    pub run: Option<String>,
    pub session: Option<SessionRef>,
    pub transcript: Option<PathBuf>,
    pub ended: bool,
}

#[derive(Deserialize)]
pub(crate) struct HostRecord {
    pub pid: u32,
    #[serde(rename = "sessionFile")]
    pub session_file: PathBuf,
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("parse {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("open {}", path.display())),
    }
}

pub(crate) fn host_record(dir: &Path) -> Result<Option<HostRecord>> {
    read_json(&dir.join("session.json"))
}

pub(crate) fn is_chat(id: &str) -> bool {
    id == QUICK_THREAD || id.starts_with("A-")
}

pub(crate) fn run(engine: &Engine, scope: &str) -> Result<Run> {
    if let Some(task) = scope.strip_suffix("/reviewer") {
        // Use the same bounds and task-id validation as an Agents row.
        views::run_scope(&format!("{task}_R1"))?;
        return engine
            .store
            .latest_run(task, Role::Reviewer)?
            .context(NotFound("reviewer session not found"));
    }
    let normalized;
    let scope = if let Some((task, run)) = scope.split_once('/') {
        normalized = format!("{task}_{run}");
        &normalized
    } else {
        scope
    };
    let (task, run) = views::run_scope(scope)?;
    engine
        .store
        .get_run(task, run)?
        .context(NotFound("task session not found"))
}

pub(crate) fn resolve(shared: &Shared, scope: &str) -> Result<Session> {
    if is_chat(scope) {
        return assistant::resolve(shared, scope);
    }
    let run = run(&shared.engine()?, scope)?;
    let ended = run.ended.is_some();
    let transcript = if let Some(SessionRef::Pi { dir, .. }) = &run.session {
        host_record(dir)?.map(|host| host.session_file)
    } else {
        None
    };
    Ok(Session {
        id: scope.into(),
        role: match run.role {
            Role::Worker => "worker",
            Role::Reviewer => "reviewer",
        },
        task: Some(run.task),
        run: Some(run.id),
        transcript,
        ended,
        session: run.session,
    })
}

impl Session {
    fn pi(&self) -> Result<Client<'_>> {
        if self.ended {
            return Err(refuse(
                "session has ended; relaunch it before sending controls",
                vec![],
            ));
        }
        match self.session.as_ref() {
            Some(session @ SessionRef::Pi { .. }) => Client::for_session(session),
            Some(_) => Err(refuse(
                "session has no Pi socket; use Terminate to stop its host",
                vec![],
            )),
            None => Err(refuse(
                "session has no running Pi host; relaunch it first",
                vec![],
            )),
        }
    }

    pub(crate) fn transcript_view(&self) -> (Option<PathBuf>, Option<String>) {
        match self
            .pi()
            .and_then(|client| client.hello_with_timeout(Duration::from_millis(250)))
        {
            Ok(hello) if !hello.session_file.is_empty() => {
                (Some(PathBuf::from(hello.session_file)), None)
            }
            Ok(_) => (self.transcript.clone(), None),
            Err(error) => (self.transcript.clone(), Some(format!("{error:#}"))),
        }
    }
}

pub(crate) fn cancel(shared: &Shared, request: SessionCancel) -> Result<SessionCancelReply> {
    let session = resolve(shared, &request.id)?;
    let client = session.pi()?;
    let reply = match request.turn {
        None => serde_json::from_value(client.cancel()?).context("decode Pi cancel reply")?,
        Some(turn) => client.request("cancel", json!({ "turn": turn }))?,
    };
    shared.publish();
    Ok(reply)
}

pub(crate) fn pause(shared: &Shared, id: &str) -> Result<SessionPauseReply> {
    let session = resolve(shared, id)?;
    let client = session.pi()?;
    if session.role == "reviewer" {
        return Err(refuse(
            "reviewers support Cancel and Terminate, not Pause",
            vec![],
        ));
    }
    let reply = serde_json::from_value(client.pause()?).context("decode Pi pause reply")?;
    shared.publish();
    Ok(reply)
}

pub(crate) fn resume(shared: &Shared, id: &str) -> Result<SessionResumeReply> {
    let reply = serde_json::from_value(resolve(shared, id)?.pi()?.resume()?)
        .context("decode Pi resume reply")?;
    shared.publish();
    Ok(reply)
}

pub(crate) fn terminate(shared: &Shared, id: &str) -> Result<()> {
    if is_chat(id) {
        return assistant::terminate(shared, id);
    }
    let engine = shared.engine()?;
    let run = run(&engine, id)?;
    engine.stop_run(&run.task, &run.id)?;
    shared.publish();
    shared.nudge(None);
    Ok(())
}

pub(crate) fn relaunch(shared: &Shared, id: &str) -> Result<SessionRelaunched> {
    if is_chat(id) {
        return assistant::relaunch(shared, id);
    }
    let engine = shared.engine()?;
    let run = run(&engine, id)?;
    let next = match run.role {
        Role::Worker => engine.retry(&run.task, Some(&run.profile), false, None)?,
        Role::Reviewer => engine.review(&run.task, Some(&run.profile))?,
    };
    shared.publish();
    shared.nudge(None);
    Ok(SessionRelaunched {
        id: format!("{}_{}", next.task, next.id),
    })
}

pub(crate) fn rewind(shared: &Shared, request: SessionRewind) -> Result<RewindReply> {
    let session = resolve(shared, &request.id)?;
    let client = session.pi()?;
    match request.action {
        RewindAction::List => {
            #[derive(Deserialize)]
            struct List {
                checkpoints: Vec<RewindCheckpoint>,
                rewinds: Vec<Rewound>,
            }
            let List {
                checkpoints,
                rewinds,
            } = client.request("rewind", json!({"list": true}))?;
            Ok(RewindReply::List {
                checkpoints,
                rewinds,
            })
        }
        RewindAction::Turn { turn } => {
            let result = client.request("rewind", json!({"turn": turn}))?;
            shared.publish();
            Ok(RewindReply::Turn(result))
        }
    }
}

pub(crate) fn answer(
    shared: &Shared,
    request: PromptAnswerRequest,
) -> Result<SessionPromptAnswered> {
    let request = match request {
        PromptAnswerRequest::Session(request) => request,
        PromptAnswerRequest::Legacy(request) => {
            let (session, question) = request
                .key
                .rsplit_once('/')
                .context("invalid: Pi prompt.answer requires the addressed session and question")?;
            let option = match request.choice {
                PromptChoice::Once => "once",
                PromptChoice::Hour => "hour",
                PromptChoice::Session => "session",
                PromptChoice::Deny => "deny",
                PromptChoice::Confirm => "confirm",
                PromptChoice::Decline => "decline",
                PromptChoice::Other => {
                    return Err(refuse("unknown option for this Pi question", vec![]));
                }
            };
            SessionPromptAnswer {
                session: session.into(),
                question: question.into(),
                option: Some(option.into()),
                text: None,
            }
        }
    };
    let answer = match (request.option.as_deref(), request.text.as_deref()) {
        (Some(option), Some(text)) => Answer::OptionWithNote { option, text },
        (Some(option), None) => Answer::Option(option),
        (None, Some(text)) => Answer::Text(text),
        (None, None) => anyhow::bail!("invalid: answer requires an option or text"),
    };
    let reply = resolve(shared, &request.session)?
        .pi()?
        .answer(&request.question, answer)?;
    shared.publish();
    Ok(SessionPromptAnswered {
        status: reply.status,
    })
}

pub(crate) fn available_for_pane(session: &SessionRef) -> &SessionRef {
    match session {
        SessionRef::Pi { inner, .. } => available_for_pane(inner),
        other => other,
    }
}

pub(crate) fn attachment_scope(entry: &str) -> Result<&str> {
    let (scope, _) = entry
        .split_once('/')
        .context("invalid: attachment must name its chat entry")?;
    ensure!(
        is_chat(scope) && scope != QUICK_THREAD,
        "invalid: attachment entry must name its chat session"
    );
    Ok(scope)
}

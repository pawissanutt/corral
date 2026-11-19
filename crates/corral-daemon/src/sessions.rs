//! Resolve wire session scopes to engine runs or daemon-owned chat directories.
use std::{
    fs::File,
    io::{BufReader, ErrorKind},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    Engine, Role, Run, SessionRef, refuse,
    runner::{
        InputTag,
        pi::{self, Client, Disposition, PiRefusal, SubmitReply},
    },
};
use corral_protocol::{
    assistant::QUICK_THREAD,
    detail::HostPresence,
    session::*,
    session_host::{InputDisposition, SendPolicy, SessionSend, SessionSent},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;

use crate::{
    assistant, builds,
    state::Shared,
    views::{self, NotFound},
};

pub(crate) mod forward;

pub(crate) struct Session {
    pub id: String,
    pub role: &'static str,
    pub task: Option<String>,
    pub run: Option<String>,
    pub dir: Option<PathBuf>,
    pub session: Option<SessionRef>,
    pub transcript: Option<PathBuf>,
    pub ended: bool,
    pub unreachable: bool,
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

pub(crate) fn is_build(id: &str) -> bool {
    pi::is_build_id(id)
}

pub(crate) fn child_scope(id: &str) -> Result<Option<(&str, &str)>> {
    let Some((parent, child)) = id.split_once("/child/") else {
        return Ok(None);
    };
    ensure!(
        (1..=128).contains(&child.len())
            && !matches!(child, "." | "..")
            && child
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)),
        "invalid: expected a child id"
    );
    if parent.starts_with("B-") {
        ensure!(is_build(parent), "invalid: expected build id B-<base36>");
    } else if is_chat(parent) {
        ensure!(
            parent == QUICK_THREAD || assistant::number(parent).is_some(),
            "invalid: expected chat id A-<number> or quick"
        );
    } else {
        run_target(parent)?;
    }
    Ok(Some((parent, child)))
}

fn run_target(scope: &str) -> Result<(&str, Option<&str>)> {
    if let Some(task) = scope.strip_suffix("/reviewer") {
        // Use the same bounds and task-id validation as an Agents row.
        views::run_scope(&format!("{task}_R1"))?;
        return Ok((task, None));
    }
    let separator = if scope.contains('/') { '/' } else { '_' };
    let (task, run) = views::run_scope_with_separator(scope, separator)?;
    Ok((task, Some(run)))
}

pub(crate) fn run(engine: &Engine, scope: &str) -> Result<Run> {
    let (task, run) = run_target(scope)?;
    let Some(run) = run else {
        return engine
            .store
            .latest_run(task, Role::Reviewer)?
            .context(NotFound("reviewer session not found"));
    };
    engine
        .store
        .get_run(task, run)?
        .context(NotFound("task session not found"))
}

pub(crate) fn resolve(shared: &Shared, scope: &str) -> Result<Session> {
    if let Some((parent, child)) = child_scope(scope)? {
        return resolve_child(resolve(shared, parent)?, child);
    }
    if scope.starts_with("B-") {
        return builds::resolve(shared, scope);
    }
    if is_chat(scope) {
        return assistant::resolve(shared, scope);
    }
    let run = run(&shared.engine()?, scope)?;
    let ended = run.ended.is_some();
    let (dir, transcript) = if let Some(SessionRef::Pi { dir, .. }) = &run.session {
        (
            Some(dir.clone()),
            host_record(dir)?.map(|host| host.session_file),
        )
    } else {
        (None, None)
    };
    Ok(Session {
        id: format!("{}_{}", run.task, run.id),
        role: match run.role {
            Role::Worker => "worker",
            Role::Reviewer => "reviewer",
        },
        task: Some(run.task),
        run: Some(run.id),
        dir,
        transcript,
        ended,
        unreachable: false,
        session: run.session,
    })
}

fn resolve_child(parent: Session, child: &str) -> Result<Session> {
    #[derive(Deserialize, serde::Serialize)]
    struct ChildFile {
        session_file: PathBuf,
    }
    #[derive(Deserialize)]
    struct Journal {
        children: Vec<JournalChild>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct JournalChild {
        id: String,
        session_file: Option<PathBuf>,
    }
    let missing = NotFound("child session not found");
    let dir = parent.dir.as_deref().context(missing)?;
    let dir = dir
        .canonicalize()
        .with_context(|| format!("resolve parent session {}", dir.display()))?;
    let safe = |file: PathBuf| {
        if !file.is_absolute() {
            return None;
        }
        let file = file.canonicalize().ok()?;
        (file.extension().is_some_and(|ext| ext == "jsonl")
            && file.starts_with(&dir)
            && file.is_file())
        .then_some(file)
    };
    let file = forward::host_call_on::<ChildFile>(
        &parent,
        "child_file",
        json!({"child": child}),
        forward::HostCall::Read,
    )
    .ok()
    .and_then(|reply| safe(reply.session_file))
    .or_else(|| {
        let journal = open_session_file(&dir, &dir.join("journal.json")).ok()?;
        let journal: Journal = serde_json::from_reader(BufReader::new(journal)).ok()?;
        let row = journal.children.into_iter().find(|row| row.id == child)?;
        safe(row.session_file?)
    })
    .or_else(|| safe(dir.join("children").join(format!("{child}.jsonl"))))
    .context(NotFound("child session not found"))?;
    Ok(Session {
        id: format!("{}/child/{child}", parent.id),
        role: "child",
        task: parent.task,
        run: parent.run,
        dir: Some(dir),
        session: None,
        transcript: Some(file),
        ended: false,
        unreachable: false,
    })
}

/// An earlier canonicalization is not sufficient if a child directory is renamed
/// or replaced between resolution and readback. No component may be a symlink.
pub(crate) fn open_session_file(dir: &Path, file: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let relative = file
        .strip_prefix(dir)
        .context("file lies outside its session directory")?;
    let mut components = relative.components().peekable();
    let mut at = open(dir, directory, Mode::empty()).context("open parent session directory")?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            anyhow::bail!("invalid path component below session directory");
        };
        if components.peek().is_none() {
            let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let file = File::from(
                openat(&at, name, flags, Mode::empty())
                    .with_context(|| format!("open session file {}", file.display()))?,
            );
            ensure!(
                file.metadata().context("inspect session file")?.is_file(),
                "session file is not regular"
            );
            return Ok(file);
        }
        at = openat(&at, name, directory, Mode::empty())
            .with_context(|| format!("open directory below {}", dir.display()))?;
    }
    anyhow::bail!("session file path names its directory")
}

pub(crate) fn valid_input_id(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

pub(crate) fn send(shared: &Shared, device: &str, request: SessionSend) -> Result<SessionSent> {
    ensure!(
        valid_input_id(&request.cmd_id),
        "invalid: expected an input id of 1 to 80 ASCII identifier bytes"
    );
    ensure!(
        request.text.len() <= corral_protocol::assistant::MESSAGE_TEXT_MAX
            && (!request.text.trim().is_empty() || !request.attachments.is_empty()),
        "invalid: send a message or an attachment within the message limit"
    );
    ensure!(
        matches!(request.policy, SendPolicy::Steer | SendPolicy::FollowUp),
        "invalid: unknown send policy"
    );
    ensure!(
        request.typed_at.as_ref().is_none_or(|at| at.len() <= 64),
        "invalid: typed_at exceeds 64 bytes"
    );
    refuse_child(&request.id)?;
    if is_chat(&request.id) {
        return assistant::send_session(shared, device, &request);
    }
    let session = resolve(shared, &request.id)?;
    session.pi_endpoint()?;
    let placement = session
        .session
        .as_ref()
        .context("Pi session placement disappeared")?;
    send_on(
        shared,
        device,
        &session.id,
        placement,
        session
            .task
            .as_deref()
            .zip(session.run.as_deref())
            .filter(|_| session.role == "worker" && request.to.is_none()),
        &request,
    )
}

pub(crate) fn send_on(
    shared: &Shared,
    device: &str,
    id: &str,
    placement: &SessionRef,
    worker: Option<(&str, &str)>,
    request: &SessionSend,
) -> Result<SessionSent> {
    let SessionRef::Pi { dir, .. } = placement else {
        return Err(no_pi_host());
    };
    let files = assistant::attachments::prepare(shared, device, dir, &request.attachments)?;
    let tag = InputTag {
        input_id: &request.cmd_id,
        to: request.to.as_deref(),
        typed_at: request.typed_at.as_deref(),
        device: Some(device),
        follow_up: request.policy == SendPolicy::FollowUp,
        attachments: &files,
    };
    let result = if let Some((task, run)) = worker {
        shared.engine()?.reply_tagged(task, run, &request.text, tag)
    } else {
        Client::for_session(placement)?.submit_input_tagged(
            &request.text,
            corral_types::Origin::Owner,
            tag,
        )
    }
    .map_err(|error| match error.downcast_ref::<PiRefusal>() {
        Some(refusal) => refuse(&refusal.reason, vec![]),
        None if builds::disconnected(&error) => no_pi_host(),
        None => error,
    })?;
    let SubmitReply {
        disposition, turn, ..
    } = result;
    let disposition = match disposition {
        Disposition::Started => InputDisposition::Started,
        Disposition::Queued => InputDisposition::Queued,
        Disposition::Handled => InputDisposition::Handled,
        Disposition::Pending => InputDisposition::Pending,
        Disposition::Rejected => return Err(refuse("Pi rejected input", vec![])),
    };
    shared.publish();
    if worker.is_some() {
        shared.nudge(None);
    }
    Ok(SessionSent {
        session: id.into(),
        entry: assistant::input_entry_id(id, &request.cmd_id),
        disposition,
        turn,
    })
}

fn refuse_child(id: &str) -> Result<()> {
    if child_scope(id)?.is_some() {
        return Err(refuse(
            "child transcripts are read-only; address the parent session",
            vec![],
        ));
    }
    Ok(())
}

fn no_pi_host() -> anyhow::Error {
    refuse("session has no running Pi host; relaunch it first", vec![])
}

impl Session {
    pub(crate) fn pi(&self) -> Result<Client<'_>> {
        let (socket, dir) = self.pi_endpoint()?;
        Ok(Client::with_dir(socket, dir))
    }

    fn pi_endpoint(&self) -> Result<(&Path, &Path)> {
        refuse_child(&self.id)?;
        if self.ended {
            return Err(refuse(
                "session has ended; relaunch it before sending controls",
                vec![],
            ));
        }
        match self.session.as_ref() {
            Some(SessionRef::Pi { socket, dir, .. }) => Ok((socket, dir)),
            Some(_) => Err(refuse(
                "session has no Pi socket; use Terminate to stop its host",
                vec![],
            )),
            None => Err(no_pi_host()),
        }
    }

    pub(crate) fn transcript_scope(&self) -> Option<&Path> {
        self.dir.as_deref().filter(|_| self.role == "child")
    }

    pub(crate) fn is_empty_quick(&self) -> bool {
        self.id == QUICK_THREAD
    }

    pub(crate) fn transcript_view(&self) -> (Option<PathBuf>, HostPresence, Option<String>) {
        if self.is_empty_quick() {
            return (None, HostPresence::NotRunning, None);
        }
        if self.role == "build" && self.session.is_some() {
            // Build resolution already observed hello and validated the socket's identity.
            return (self.transcript.clone(), HostPresence::Running, None);
        }
        let client = match self.pi() {
            Ok(client) => client,
            Err(error) => {
                let host = if self.unreachable {
                    HostPresence::Unreachable
                } else {
                    HostPresence::NotRunning
                };
                return (self.transcript.clone(), host, Some(format!("{error:#}")));
            }
        };
        match client.hello_with_timeout(Duration::from_millis(250)) {
            Ok(hello) => {
                let path = if hello.session_file.is_empty() {
                    self.transcript.clone()
                } else {
                    Some(hello.session_file.into())
                };
                (path, HostPresence::Running, None)
            }
            Err(error) => {
                let host = if builds::disconnected(&error) {
                    HostPresence::NotRunning
                } else {
                    HostPresence::Unreachable
                };
                (self.transcript.clone(), host, Some(format!("{error:#}")))
            }
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
    refuse_child(id)?;
    if id.starts_with("B-") {
        return builds::terminate(shared, id);
    }
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
    refuse_child(id)?;
    if id.starts_with("B-") {
        return builds::relaunch(shared, id);
    }
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

pub(crate) fn available_for_pane(session: &SessionRef) -> &SessionRef {
    match session {
        SessionRef::Pi { inner, .. } => available_for_pane(inner),
        other => other,
    }
}

pub(crate) fn attachment_scope(entry: &str) -> Result<&str> {
    let (scope, suffix) = entry
        .rsplit_once('/')
        .and_then(|(head, suffix)| {
            head.rsplit_once('/')
                .map(|(scope, kind)| (scope, (kind, suffix)))
        })
        .context("invalid: attachment must name a session message")?;
    let (kind, key) = suffix;
    let scope = if let Some(base) = scope.strip_suffix("/replay") {
        ensure!(
            !kind.is_empty() && !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()),
            "invalid: malformed replay id"
        );
        base
    } else {
        ensure!(
            matches!(kind, "entry" | "input") && !key.is_empty(),
            "invalid: malformed message id"
        );
        scope
    };
    ensure!(
        scope != QUICK_THREAD && child_scope(scope)?.is_none(),
        "invalid: attachment must name a concrete parent session"
    );
    Ok(scope)
}

//! The quick thread and named chats are Pi sessions, not a separate message store.
use std::{
    fs::{self, DirBuilder},
    io::{ErrorKind, Write},
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use corral_engine::{
    Config, Runner, refuse,
    runner::pi::{self, ChatLaunch, Client, PiRunner},
};
use corral_protocol::{
    assistant::*,
    rows::*,
    session::SessionRelaunched,
    session_host::{SessionSend, SessionSent},
};
use corral_types::SessionRef;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{builds, sessions, state::Shared, views::NotFound};

pub(crate) mod attachments;
pub(crate) mod automation;

#[derive(Default)]
pub(crate) struct Service {
    // Serialize creation and mutations. Readers observe atomically replaced chat files.
    mutation: Mutex<()>,
    uploads: Mutex<()>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Meta {
    quick: bool,
    profile: String,
    task: Option<String>,
    ended: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automation: Option<automation::Binding>,
}

struct Chat {
    number: u64,
    dir: PathBuf,
    meta: Meta,
}

#[derive(Deserialize)]
struct HostState {
    turn: Turn,
    paused: bool,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    owner_questions: Vec<Question>,
}

#[derive(Deserialize)]
struct Turn {
    current: u64,
    open: bool,
}

#[derive(Deserialize)]
struct Question {
    id: Option<String>,
    question: String,
}

fn root(shared: &Shared) -> PathBuf {
    shared.root().join(crate::STATE_DIR).join("chat")
}

fn socket_root(shared: &Shared) -> Option<PathBuf> {
    // An injected runner uses the fixture workspace's private socket directory.
    shared.private_chat_sockets()
}

pub(crate) fn number(id: &str) -> Option<u64> {
    let s = id.strip_prefix("A-")?;
    let number = s.parse().ok()?;
    (number != 0 && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit())).then_some(number)
}

fn records(shared: &Shared) -> Result<Vec<Chat>> {
    let path = root(shared);
    let dir = match fs::read_dir(&path) {
        Ok(dir) => dir,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let mut chats = Vec::new();
    for file in dir {
        let file = file.context("read chat directory entry")?;
        let Some(n) = file.file_name().to_str().and_then(number) else {
            continue;
        };
        let path = file.path();
        if let Some(meta) = sessions::read_json(&path.join("chat.json"))? {
            chats.push(Chat {
                number: n,
                dir: path,
                meta,
            });
        }
    }
    chats.sort_unstable_by_key(|chat| chat.number);
    Ok(chats)
}

fn lookup(shared: &Shared, scope: &str) -> Result<Option<Chat>> {
    if scope == QUICK_THREAD {
        let mut quicks = records(shared)?
            .into_iter()
            .rev()
            .filter(|chat| chat.meta.quick);
        let latest = quicks.next();
        if latest.as_ref().is_some_and(|chat| chat.meta.ended) {
            return Ok(quicks.find(|chat| !chat.meta.ended).or(latest));
        }
        return Ok(latest);
    }
    let n = number(scope).context("invalid: expected chat id A-<number> or quick")?;
    let dir = root(shared).join(scope);
    Ok(
        sessions::read_json(&dir.join("chat.json"))?.map(|meta| Chat {
            number: n,
            dir,
            meta,
        }),
    )
}

fn require(shared: &Shared, scope: &str) -> Result<Chat> {
    lookup(shared, scope)?
        .filter(|chat| scope != QUICK_THREAD || !chat.meta.ended)
        .context(NotFound("chat session not found"))
}

impl Chat {
    fn id(&self) -> String {
        format!("A-{}", self.number)
    }

    fn save(&self) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.dir).context("create chat record")?;
        serde_json::to_writer(&mut file, &self.meta).context("write chat record")?;
        file.flush().context("flush chat record")?;
        file.persist(self.dir.join("chat.json"))
            .context("replace chat record")?;
        Ok(())
    }

    fn resolve(&self, shared: &Shared) -> Result<sessions::Session> {
        let host = sessions::host_record(&self.dir)?;
        let placement = host
            .as_ref()
            .filter(|_| !self.meta.ended)
            .map(|host| pi::chat_session(&self.dir, host.pid, socket_root(shared).as_deref()))
            .transpose()?;
        Ok(sessions::Session {
            id: self.id(),
            role: "assistant",
            task: self.meta.task.clone(),
            run: None,
            dir: Some(self.dir.clone()),
            session: placement,
            transcript: host.map(|host| host.session_file),
            ended: self.meta.ended,
            unreachable: false,
        })
    }

    fn start(&self, shared: &Shared, config: &Config) -> Result<SessionRef> {
        ensure!(!self.meta.ended, "chat has ended");
        let automation = automation::launch_policy(self, config)?;
        let (harness, profile) = profile(config, &self.meta.profile)?;
        let project = self
            .meta
            .task
            .as_deref()
            .map(|task| -> Result<_> { Ok(shared.engine()?.store.require_task(task)?.project) })
            .transpose()?
            .flatten();
        let (session, hello) =
            PiRunner::new(shared.chat_placement().as_ref()).start_chat(ChatLaunch {
                workspace: shared.root(),
                dir: &self.dir,
                socket_root: socket_root(shared).as_deref(),
                harness,
                profile,
                task: self.meta.task.as_deref(),
                project: project.as_deref(),
                automation,
            })?;
        ensure!(
            hello.session_id == self.id(),
            "Pi socket answers for a different chat session"
        );
        shared.publish();
        Ok(session)
    }

    fn stop(&self, shared: &Shared) -> Result<()> {
        if let Some(session) = self.resolve(shared)?.session {
            let client = Client::for_session(&session)?;
            // No process action from a stale pid recorded by a host that has already exited.
            if let Some(hello) = client.hello_if_available()? {
                ensure!(
                    hello.session_id == self.id(),
                    "Pi socket answers for a different chat session"
                );
                let session =
                    pi::chat_session(&self.dir, hello.pid, socket_root(shared).as_deref())?;
                PiRunner::new(shared.chat_placement().as_ref()).stop(&session)?;
            }
        }
        shared.publish();
        Ok(())
    }
}

pub(crate) fn resolve(shared: &Shared, scope: &str) -> Result<sessions::Session> {
    match lookup(shared, scope)? {
        Some(chat) if scope != QUICK_THREAD || !chat.meta.ended => chat.resolve(shared),
        _ if scope == QUICK_THREAD => Ok(sessions::Session {
            id: QUICK_THREAD.into(),
            role: "assistant",
            task: None,
            run: None,
            dir: None,
            session: None,
            transcript: None,
            ended: false,
            unreachable: false,
        }),
        _ => Err(NotFound("chat session not found").into()),
    }
}

/// Built-ins alone are not an opt-in. A Pi harness override or any named Pi
/// profile in corral.toml enables owner chat, independent of automation.
pub(crate) fn configured(shared: &Shared) -> Result<bool> {
    let text = crate::config::get(shared)?.toml;
    let doc: toml::Table = toml::from_str(&text).context("parse chat configuration")?;
    Ok(doc
        .get("harnesses")
        .and_then(toml::Value::as_table)
        .is_some_and(|harnesses| harnesses.contains_key("pi"))
        || doc
            .get("profiles")
            .and_then(toml::Value::as_table)
            .is_some_and(|profiles| {
                profiles.values().any(|profile| {
                    profile
                        .as_table()
                        .and_then(|profile| profile.get("harness"))
                        .and_then(toml::Value::as_str)
                        == Some("pi")
                })
            })
        || doc
            .get("defaults")
            .and_then(toml::Value::as_table)
            .and_then(|defaults| defaults.get("profile"))
            .and_then(toml::Value::as_str)
            == Some("pi"))
}

fn load_config(shared: &Shared) -> Result<Config> {
    Config::load(&shared.root().join(crate::STATE_DIR).join("corral.toml"))
}

fn owner_config(shared: &Shared, scope: &str) -> Result<Config> {
    if lookup(shared, scope)?.is_some_and(|chat| chat.meta.automation.is_some()) {
        return load_config(shared);
    }
    config(shared)
}

fn config(shared: &Shared) -> Result<Config> {
    if !configured(shared)? {
        return Err(refuse(
            "chat requires a configured Pi harness or profile in corral.toml",
            vec![],
        ));
    }
    load_config(shared)
}

fn profile<'a>(
    config: &'a Config,
    name: &str,
) -> Result<(
    &'a corral_engine::config::Harness,
    &'a corral_engine::config::Profile,
)> {
    let Some(profile) = config
        .profiles
        .get(name)
        .filter(|profile| profile.harness == "pi")
    else {
        return Err(refuse(
            format!("chat profile {name:?} must use the Pi harness"),
            vec![],
        ));
    };
    let harness = config
        .harnesses
        .get("pi")
        .context("Pi harness is not configured")?;
    Ok((harness, profile))
}

fn default_profile(config: &Config) -> &str {
    let is_pi = |name: &str| config.profiles.get(name).is_some_and(|p| p.harness == "pi");
    if let Some(name) = config
        .defaults
        .profile
        .as_deref()
        .filter(|name| is_pi(name))
    {
        name
    } else {
        config
            .profiles
            .iter()
            .find(|(name, p)| {
                p.harness == "pi"
                    && (*name != "pi"
                        || p.model.is_some()
                        || p.effort.is_some()
                        || !p.args.is_empty()
                        || p.fallback
                            .as_ref()
                            .is_some_and(|fallback| !fallback.is_empty()))
            })
            .map_or("pi", |(name, _)| name.as_str())
    }
}

fn create(
    shared: &Shared,
    quick: bool,
    task: Option<String>,
    profile: String,
    automation: Option<automation::Binding>,
) -> Result<Chat> {
    if let Some(task) = &task {
        shared.engine()?.store.require_task(task)?;
    }
    let root = root(shared);
    fs::create_dir_all(&root).context("create workspace chat directory")?;
    let mut n = records(shared)?.last().map_or(0, |chat| chat.number);
    loop {
        n = n.checked_add(1).context("chat id space exhausted")?;
        let dir = root.join(format!("A-{n}"));
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {
                let chat = Chat {
                    number: n,
                    dir,
                    meta: Meta {
                        quick,
                        profile,
                        task,
                        ended: false,
                        automation,
                    },
                };
                chat.save()?;
                shared.publish();
                return Ok(chat);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).with_context(|| format!("create {}", dir.display())),
        }
    }
}

fn current(shared: &Shared, config: &Config, scope: &str) -> Result<Chat> {
    match lookup(shared, scope)? {
        Some(chat) if !chat.meta.ended => Ok(chat),
        Some(chat) if chat.meta.automation.is_some() => Err(refuse(
            "automation chat ended; the rule will allocate a new chat when it fires",
            vec![],
        )),
        Some(chat) => create(
            shared,
            chat.meta.quick,
            chat.meta.task,
            chat.meta.profile,
            None,
        ),
        None if scope == QUICK_THREAD => {
            create(shared, true, None, default_profile(config).into(), None)
        }
        None => Err(NotFound("chat session not found").into()),
    }
}

impl Chat {
    fn row(self, shared: &Shared) -> Result<AssistantRow> {
        let chat = self;
        let resolved = chat.resolve(shared)?;
        let mut status_unavailable = false;
        let host = resolved.session.as_ref().and_then(|session| {
            let client = Client::for_session(session).ok()?;
            match client.request_with_timeout::<_, HostState>(
                "get_state",
                json!({}),
                Duration::from_millis(250),
            ) {
                Ok(host) => Some(host),
                Err(error) => {
                    status_unavailable =
                        error
                            .downcast_ref::<pi::PiRefusal>()
                            .is_some_and(|refusal| {
                                refusal.command == "get_state"
                                    && refusal.code == "response_too_large"
                            });
                    None
                }
            }
        });
        let state = if chat.meta.ended {
            AssistantState::Ended
        } else if let Some(host) = &host {
            if host.paused {
                AssistantState::Paused
            } else if !host.owner_questions.is_empty() {
                AssistantState::Waiting
            } else if host.turn.open {
                AssistantState::Working
            } else if host.last_error.is_some() {
                AssistantState::Failed
            } else {
                AssistantState::Ready
            }
        } else if status_unavailable {
            AssistantState::Other
        } else {
            AssistantState::Cold
        };
        let live_in = if host.is_some() || status_unavailable {
            AssistantLocation::Daemon
        } else {
            AssistantLocation::None
        };
        let turn = host
            .as_ref()
            .map(|host| host.turn.current)
            .filter(|turn| *turn > 0);
        let unavailable = status_unavailable.then(|| WaitReason {
            code: "status_unavailable".into(),
            text: "Pi status unavailable: response too large".into(),
            retry_at: None,
        });
        let (wait, question_id) = host.map_or((unavailable, None), |host| {
            if let Some(q) = host.owner_questions.into_iter().next() {
                let wait = WaitReason {
                    code: "owner_question".into(),
                    text: q.question,
                    retry_at: None,
                };
                (
                    Some(wait),
                    q.id.filter(|_| state == AssistantState::Waiting),
                )
            } else {
                let wait = host.last_error.map(|text| WaitReason {
                    code: "host_error".into(),
                    text,
                    retry_at: None,
                });
                (wait, None)
            }
        });
        Ok(AssistantRow {
            id: chat.id(),
            kind: if chat.meta.quick {
                AssistantKind::Quick
            } else {
                AssistantKind::Session
            },
            title: if chat.meta.quick {
                "Quick thread".into()
            } else if let Some(binding) = &chat.meta.automation {
                format!("Automation: {}", binding.rule)
            } else {
                format!("Chat {}", chat.number)
            },
            state,
            live_in,
            profile: Some(chat.meta.profile),
            connection: None,
            focus: chat.meta.task,
            pending: 0,
            turn,
            cost_usd: None,
            cost_basis: None,
            wait,
            question_id,
            rev: shared.revision(),
            model: None,
            effort: None,
            created: None,
            updated: None,
            build: None,
        })
    }
}

pub(crate) fn rows(shared: &Shared, include_builds: bool) -> Result<Vec<AssistantRow>> {
    let mut rows = records(shared)?
        .into_iter()
        .map(|chat| chat.row(shared))
        .collect::<Result<Vec<_>>>()?;
    if include_builds {
        rows.extend(builds::rows(shared)?);
    }
    Ok(rows)
}

pub(crate) fn input_entry_id(session: &str, request_id: &str) -> String {
    format!("{session}/input/{request_id}")
}

fn start_on_send(shared: &Shared, scope: &str) -> Result<(Chat, SessionRef)> {
    let config = owner_config(shared, scope)?;
    let chat = current(shared, &config, scope)?;
    automation::require_resumed(&chat)?;
    let session = chat.start(shared, &config)?;
    Ok((chat, session))
}

pub(crate) fn send_session(
    shared: &Shared,
    device: &str,
    request: &SessionSend,
) -> Result<SessionSent> {
    let _lock = shared
        .chat
        .mutation
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (chat, session) = start_on_send(shared, &request.id)?;
    sessions::send_on(shared, device, &chat.id(), &session, None, request)
}

pub(crate) fn warm(shared: &Shared, request: AssistantWarm) -> Result<()> {
    let _lock = shared
        .chat
        .mutation
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let config = owner_config(shared, &request.thread)?;
    let chat = current(shared, &config, &request.thread)?;
    automation::require_resumed(&chat)?;
    chat.start(shared, &config)?;
    Ok(())
}

pub(crate) fn terminate(shared: &Shared, id: &str) -> Result<()> {
    let _lock = shared
        .chat
        .mutation
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut chat = require(shared, id)?;
    automation::set_stopped(shared, &mut chat, true)?;
    chat.stop(shared)
}

pub(crate) fn relaunch(shared: &Shared, id: &str) -> Result<SessionRelaunched> {
    let _lock = shared
        .chat
        .mutation
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut chat = require(shared, id)?;
    if chat.meta.ended {
        return Err(refuse(
            "chat thread ended; send a message to start a new session",
            vec![],
        ));
    }
    chat.start(shared, &owner_config(shared, id)?)?;
    automation::set_stopped(shared, &mut chat, false)?;
    Ok(SessionRelaunched { id: chat.id() })
}

pub(crate) fn session(shared: &Shared, operation: AssistantSessionOp) -> Result<AssistantRow> {
    match operation {
        AssistantSessionOp::New {
            task,
            profile: choice,
        } => {
            let _lock = shared
                .chat
                .mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let config = config(shared)?;
            let choice = choice.unwrap_or_else(|| default_profile(&config).into());
            profile(&config, &choice)?;
            let chat = create(shared, false, task, choice, None)?;
            chat.start(shared, &config)?;
            chat.row(shared)
        }
        AssistantSessionOp::End { session } => {
            let _lock = shared
                .chat
                .mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let mut chat = require(shared, &session)?;
            chat.stop(shared)?;
            chat.meta.ended = true;
            chat.save()?;
            shared.publish();
            chat.row(shared)
        }
        AssistantSessionOp::SetProfile {
            thread,
            profile: choice,
        } => {
            let _lock = shared
                .chat
                .mutation
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let config = owner_config(shared, &thread)?;
            profile(&config, &choice)?;
            let mut chat = current(shared, &config, &thread)?;
            if chat.meta.profile != choice {
                if chat.meta.automation.is_some() {
                    return Err(refuse(
                        "change the automation agent profile in corral.toml",
                        vec![],
                    ));
                }
                let active = chat.resolve(shared)?.session;
                let mut restart = false;
                if let Some(session) = &active {
                    let client = Client::for_session(session)?;
                    if client.hello_if_available()?.is_some() {
                        let state: HostState = client.request("get_state", json!({}))?;
                        if state.turn.open {
                            return Err(refuse(
                                "wait for the current turn or cancel it before changing profile",
                                vec![],
                            ));
                        }
                        chat.stop(shared)?;
                        restart = true;
                    }
                }
                chat.meta.profile = choice;
                chat.save()?;
                if restart {
                    chat.start(shared, &config)?;
                }
                shared.publish();
            }
            chat.row(shared)
        }
        AssistantSessionOp::Other => bail!("invalid: unknown assistant session operation"),
    }
}

pub(crate) fn attach_begin(
    shared: &Shared,
    device: &str,
    request: corral_protocol::attachment::AttachmentBegin,
) -> Result<corral_protocol::attachment::UploadAck> {
    attachments::begin(shared, device, request)
}

pub(crate) fn attach_put(
    shared: &Shared,
    device: &str,
    request: corral_protocol::attachment::AttachmentPut,
) -> Result<corral_protocol::attachment::UploadAck> {
    attachments::put(shared, device, request)
}

pub(crate) fn attach_discard(
    shared: &Shared,
    device: &str,
    request: corral_protocol::attachment::AttachmentDiscard,
) -> Result<()> {
    attachments::discard(shared, device, request)
}

pub(crate) fn attachment_path(shared: &Shared, entry: &str, id: &str) -> Result<PathBuf> {
    attachments::detail_path(shared, entry, id)
}

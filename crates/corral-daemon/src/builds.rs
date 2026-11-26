//! A build's Pi directory is the durable record; the socket is proof of liveness.
use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    refuse,
    runner::pi::{Client, Hello, build_session, build_socket},
};
use corral_protocol::{
    build::{BuildDiff, BuildFacts, BuildMode},
    rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState, WaitReason},
    session_state::{Counts, ExitBrief, ExitChoice, ExitPhase, SessionMode, SessionState},
};
use corral_types::{SessionRef, SessionStatus};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    sessions::{Session, forward, is_build, read_json},
    state::Shared,
    views::NotFound,
};

mod operations;
use operations::oversized_status;
#[cfg(test)]
pub(crate) use operations::remove_with;
pub(crate) use operations::{relaunch, remove, resume_code, start, start_code, terminate};

pub(crate) fn reject_code(code: bool) -> Result<()> {
    if code {
        return Err(refuse(
            "Code sessions do not accept managed Build operations",
            vec![],
        ));
    }
    Ok(())
}

const PROBE: Duration = Duration::from_millis(250);
const MAX_ROWS: usize = 200;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: String,
    name: String,
    title: Option<String>,
    workspace_root: Option<PathBuf>,
    repo_root: String,
    worktree: String,
    project: Option<String>,
    branch: Option<String>,
    base: Option<String>,
    here: bool,
    #[serde(default)]
    bare: bool,
    #[serde(default)]
    trusted_code: bool,
    created: String,
    updated: String,
    session_file: Option<PathBuf>,
    socket: Option<PathBuf>,
    route: Option<Route>,
    exit: Option<SavedExit>,
}

#[derive(Deserialize)]
struct Route {
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedExit {
    choice: ExitChoice,
    landed: Option<String>,
    base_branch: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    session_title: Option<String>,
    title: Option<String>,
    diff: Option<BuildDiff>,
    #[serde(default)]
    children: Vec<JournalChild>,
}

fn build_title<'a>(record: &'a Record, journal: &'a Journal) -> &'a str {
    [
        journal.session_title.as_deref(),
        record.title.as_deref(),
        journal.title.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|title| !title.is_empty())
    .unwrap_or(&record.name)
}

#[derive(Deserialize)]
struct JournalChild {
    state: String,
}

struct Build {
    dir: PathBuf,
    record: Record,
}

#[derive(Deserialize)]
struct StatePage {
    state: Option<SessionState>,
}

#[derive(Default, Deserialize)]
struct LegacyState {
    mode: Option<SessionMode>,
    turn: Option<LegacyTurn>,
    #[serde(default)]
    paused: bool,
    last_error: Option<String>,
    model: Option<String>,
    #[serde(default)]
    owner_questions: Vec<LegacyQuestion>,
}

#[derive(Deserialize)]
struct LegacyTurn {
    current: u64,
    open: bool,
}

#[derive(Deserialize)]
struct LegacyQuestion {
    id: Option<String>,
    question: String,
}

impl Build {
    fn read(root: &Path, build_root: &Path, id: &str) -> Result<Option<Self>> {
        ensure!(is_build(id), "invalid: expected build id B-<base36>");
        let dir = build_root.join(id);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("inspect {}", dir.display())),
        }
        let Some(record) = read_json::<Record>(&dir.join("session.json"))? else {
            return Ok(None);
        };
        if record.bare || record.id != id {
            return Ok(None);
        }
        if !record.workspace_root.as_ref().is_some_and(|workspace| {
            workspace.is_absolute() && workspace.canonicalize().is_ok_and(|path| path == root)
        }) {
            return Ok(None);
        }
        Ok(Some(Self { dir, record }))
    }

    fn live(&self, shared: &Shared) -> Result<Option<(SessionRef, Hello)>> {
        let socket = build_socket(
            &self.dir,
            self.record.socket.as_deref(),
            shared.private_chat_sockets().as_deref(),
        )?;
        let hello = match probe::<Hello>(&socket, "hello", json!({})) {
            Ok(hello) if hello.protocol != 2 => {
                anyhow::bail!("unsupported Pi build protocol {}", hello.protocol)
            }
            Ok(hello) if hello.session_id == self.record.id && hello.role == "build" => hello,
            Ok(_) => return Ok(None),
            Err(err) if disconnected(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        Ok(Some((
            build_session(&self.dir, hello.pid, socket.into_owned()),
            hello,
        )))
    }

    fn landed(&self) -> bool {
        self.record
            .exit
            .as_ref()
            .is_some_and(|exit| exit.landed.is_some())
    }

    fn row(self, shared: &Shared, live: Option<&SessionRef>) -> AssistantRow {
        let socket = match live {
            Some(SessionRef::Pi { socket, .. }) => Some(socket.as_path()),
            _ => None,
        };
        let mut page = socket.and_then(|socket| {
            probe::<StatePage>(socket, "live_since", json!({"seq": 0}))
                .ok()?
                .state
        });
        let mut status_unavailable = false;
        let legacy = if let Some(socket) = socket.filter(|_| page.is_none()) {
            match probe(socket, "get_state", json!({})) {
                Ok(state) => state,
                Err(error) => {
                    status_unavailable = oversized_status(&error);
                    LegacyState::default()
                }
            }
        } else {
            LegacyState::default()
        };
        let LegacyState {
            mode,
            turn,
            paused,
            last_error,
            model,
            owner_questions,
        } = legacy;
        let journal = read_json::<Journal>(&self.dir.join("journal.json"))
            .ok()
            .flatten()
            .unwrap_or_default();
        let first_card = page
            .as_ref()
            .and_then(|state| state.cards.first())
            .map(|card| (Some(card.id.as_str()), card.title.as_str()))
            .or_else(|| {
                owner_questions
                    .first()
                    .map(|q| (q.id.as_deref(), q.question.as_str()))
            });
        let host_error = page
            .as_ref()
            .and_then(|state| state.last_error.as_deref())
            .or(last_error.as_deref());
        let state = if live.is_none() {
            if self.landed() {
                AssistantState::Ended
            } else {
                AssistantState::Cold
            }
        } else if status_unavailable {
            AssistantState::Other
        } else if first_card.is_some() {
            AssistantState::Waiting
        } else if page.as_ref().map_or(paused, |s| s.paused) {
            AssistantState::Paused
        } else if page
            .as_ref()
            .map_or_else(|| turn.as_ref().is_some_and(|t| t.open), |s| s.open)
        {
            AssistantState::Working
        } else if host_error.is_some() {
            AssistantState::Failed
        } else {
            AssistantState::Ready
        };
        let live_in = match (live, page.as_ref().map(|s| s.mode).or(mode)) {
            (None, _) => AssistantLocation::None,
            (Some(_), Some(SessionMode::Interactive)) => AssistantLocation::Pane,
            (Some(_), _) if status_unavailable => AssistantLocation::Other,
            (Some(_), _) => AssistantLocation::Daemon,
        };
        let wait = first_card
            .map(|(_, title)| ("owner_question", title))
            .or_else(|| host_error.map(|text| ("host_error", text)))
            .or_else(|| {
                status_unavailable.then_some((
                    "status_unavailable",
                    "Pi status unavailable: response too large",
                ))
            })
            .map(|(code, text)| WaitReason {
                code: code.into(),
                text: bounded(text, 512),
                retry_at: None,
            });
        let question_id = first_card.and_then(|(id, _)| id.map(|id| bounded(id, 160)));
        let mut children = Counts::default();
        for child in &journal.children {
            children.total += 1;
            match child.state.as_str() {
                "needs" => {
                    children.waiting += 1;
                    children.live += 1;
                }
                "queued" | "running" | "stopping" => children.live += 1,
                _ => {}
            }
        }
        let record = self.record;
        let title = bounded(build_title(&record, &journal), 160);
        let mut route = record.route;
        let saved_model = || {
            let route = route.as_ref()?;
            let provider = route.provider.as_deref()?;
            let model = route.model.as_deref()?;
            if model
                .strip_prefix(provider)
                .is_some_and(|tail| tail.starts_with('/'))
            {
                return Some(model.into());
            }
            Some(format!("{provider}/{model}"))
        };
        let model = page
            .as_mut()
            .and_then(|s| s.model.take())
            .or(model)
            .or_else(saved_model);
        let effort = page
            .as_mut()
            .and_then(|s| s.effort.take())
            .or_else(|| route.as_mut().and_then(|route| route.effort.take()));
        let exit = page.as_mut().and_then(|s| s.exit.take()).or_else(|| {
            record.exit.map(|exit| {
                let phase = if exit.landed.is_some() {
                    ExitPhase::Landed
                } else if exit.choice == ExitChoice::Keep {
                    ExitPhase::Kept
                } else {
                    ExitPhase::None
                };
                ExitBrief {
                    phase,
                    detail: None,
                    choice: Some(exit.choice),
                    landed: exit.landed,
                    base_branch: exit.base_branch,
                    repair: None,
                    land: None,
                    // The saved record names what landed, not a prepared candidate.
                    candidate: None,
                }
            })
        });
        AssistantRow {
            id: record.id,
            kind: AssistantKind::Build,
            title,
            state,
            live_in,
            profile: None,
            connection: None,
            focus: None,
            pending: page.as_ref().map_or(0, |s| s.queue_total),
            turn: page
                .as_ref()
                .map(|s| s.turn)
                .or_else(|| turn.map(|t| t.current))
                .filter(|turn| *turn > 0),
            cost_usd: page.as_ref().and_then(|s| s.usage.cost_usd),
            cost_basis: None,
            wait,
            question_id,
            rev: shared.revision(),
            model,
            effort,
            created: Some(record.created),
            updated: Some(record.updated),
            build: Some(BuildFacts {
                mode: if record.trusted_code {
                    BuildMode::Code
                } else {
                    BuildMode::Build
                },
                name: record.name,
                project: record.project,
                worktree: record.worktree,
                repo_root: record.repo_root,
                branch: record.branch,
                base: record.base,
                here: record.here,
                exit,
                children: page.map_or(children, |s| s.children),
                diff: journal.diff,
            }),
        }
    }
}

fn bounded(text: &str, bytes: usize) -> String {
    text[..text.floor_char_boundary(bytes.min(text.len()))].into()
}

fn probe<R: DeserializeOwned>(socket: &Path, command: &str, args: Value) -> Result<R> {
    let deadline = Instant::now() + PROBE;
    let stream = forward::connect(socket, deadline)
        .with_context(|| format!("connect to build socket {}", socket.display()))?;
    Client::request_on(stream, command, args, deadline)
}

pub(crate) fn disconnected(error: &anyhow::Error) -> bool {
    use rustix::io::Errno;

    matches!(
        error.downcast_ref::<Errno>(),
        Some(&Errno::NOENT | &Errno::CONNREFUSED | &Errno::CONNRESET | &Errno::PIPE)
    ) || error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            ErrorKind::NotFound
                | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::UnexpectedEof
                | ErrorKind::BrokenPipe
        )
    })
}

fn require(shared: &Shared, id: &str) -> Result<Build> {
    ensure!(is_build(id), "invalid: expected build id B-<base36>");
    require_at(shared, &shared.build_data_root()?, id)
}

fn require_at(shared: &Shared, data_root: &Path, id: &str) -> Result<Build> {
    Build::read(shared.root(), &data_root.join("build"), id)?
        .context(NotFound("build session not found"))
}

pub(crate) fn resolve(shared: &Shared, id: &str, managed_exit: bool) -> Result<Session> {
    let build = require(shared, id)?;
    if managed_exit {
        reject_code(build.record.trusted_code)?;
    }
    let role = if build.record.trusted_code {
        "code"
    } else {
        "build"
    };
    let live = build.live(shared);
    let ended = matches!(live, Ok(None)) && build.landed();
    let unreachable = live.is_err();
    let (session, transcript) = match live {
        Ok(Some((session, hello))) => {
            let transcript = if hello.session_file.is_empty() {
                build.record.session_file
            } else {
                Some(hello.session_file.into())
            };
            (Some(session), transcript)
        }
        _ => (None, build.record.session_file),
    };
    Ok(Session {
        id: build.record.id,
        role,
        task: None,
        run: None,
        dir: Some(build.dir),
        session,
        transcript,
        ended,
        unreachable,
    })
}

pub(crate) fn rows(shared: &Shared) -> Result<Vec<AssistantRow>> {
    let root = shared.build_data_root()?.join("build");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", root.display())),
    };
    // Incomplete launches and damaged or foreign records cannot hide healthy sessions.
    let mut records: Vec<_> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name();
            Build::read(shared.root(), &root, name.to_str()?)
                .ok()
                .flatten()
        })
        .collect();
    records.sort_unstable_by(|left, right| {
        (&right.record.updated, &right.record.id).cmp(&(&left.record.updated, &left.record.id))
    });
    records.truncate(MAX_ROWS);
    Ok(records
        .into_iter()
        .map(|build| {
            let live = build.live(shared).ok().flatten();
            build.row(shared, live.as_ref().map(|(session, _)| session))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_build_title_priority_and_empty_values() {
        let mut record: Record = serde_json::from_value(json!({
            "id": "B-build1", "name": "fallback", "title": "launch title",
            "repoRoot": "/repo", "worktree": "/worktree", "here": false,
            "created": "now", "updated": "now"
        }))
        .unwrap();
        let mut journal: Journal = serde_json::from_value(json!({
            "sessionTitle": " renamed in Pi ", "title": "first prompt"
        }))
        .unwrap();
        assert_eq!(build_title(&record, &journal), "renamed in Pi");
        journal.session_title = Some("  ".into());
        assert_eq!(build_title(&record, &journal), "launch title");
        record.title = None;
        assert_eq!(build_title(&record, &journal), "first prompt");
        journal.title = Some(" \n".into());
        assert_eq!(build_title(&record, &journal), "fallback");
        assert_eq!(build_title(&record, &Journal::default()), "fallback");
    }

    #[test]
    fn cold_build_row_reads_the_saved_pi_title_at_a_utf8_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let shared = Shared::new(root.clone(), None).unwrap();
        let id = "B-rename";
        let dir = root.join(id);
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("session.json"),
            json!({
                "id": id, "name": "fallback", "title": "launch title",
                "workspaceRoot": root, "repoRoot": root, "worktree": root,
                "here": true, "created": "now", "updated": "now"
            })
            .to_string(),
        )
        .unwrap();
        let prefix = "界".repeat(53);
        fs::write(
            dir.join("journal.json"),
            json!({"sessionTitle": format!("{prefix}😀"), "title": "first prompt"}).to_string(),
        )
        .unwrap();
        let row = Build::read(&root, &root, id)
            .unwrap()
            .unwrap()
            .row(&shared, None);
        assert_eq!(row.state, AssistantState::Cold);
        assert_eq!(row.title, prefix);
    }
}

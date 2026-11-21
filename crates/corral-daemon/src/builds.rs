//! A build's Pi directory is the durable record; the socket is proof of liveness.
use std::{
    fs::{self, DirBuilder, File},
    io::{self, ErrorKind, Read, Seek, SeekFrom},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use corral_engine::{
    refuse,
    runner::pi::{BuildLaunch, Client, Hello, PiRefusal, PiRunner, build_session, build_socket},
};
use corral_protocol::{
    app::SESSION_PART_MAX,
    build::{BuildDiff, BuildFacts, BuildRemove, BuildRemoved, BuildStart},
    rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState, WaitReason},
    session::SessionRelaunched,
    session_state::{Counts, ExitBrief, ExitChoice, ExitPhase, SessionMode, SessionState},
};
use corral_types::{SessionRef, SessionStatus};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    sessions::{Session, forward, is_build, read_json},
    state::Shared,
    views::{NotFound, json_len},
};

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
struct Journal {
    title: Option<String>,
    diff: Option<BuildDiff>,
    #[serde(default)]
    children: Vec<JournalChild>,
}

fn build_title<'a>(explicit: Option<&'a str>, journal: Option<&'a str>, name: &'a str) -> &'a str {
    explicit
        .filter(|title| !title.trim().is_empty())
        .or_else(|| journal.filter(|title| !title.trim().is_empty()))
        .unwrap_or(name)
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
                "queued" | "running" => children.live += 1,
                _ => {}
            }
        }
        let record = self.record;
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
                }
            })
        });
        AssistantRow {
            id: record.id,
            kind: AssistantKind::Build,
            title: bounded(
                build_title(
                    record.title.as_deref(),
                    journal.title.as_deref(),
                    &record.name,
                ),
                160,
            ),
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

pub(crate) fn resolve(shared: &Shared, id: &str) -> Result<Session> {
    let build = require(shared, id)?;
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
        role: "build",
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

pub(crate) fn start(shared: &Shared, request: BuildStart) -> Result<AssistantRow> {
    let cwd = if let Some(project) = &request.project {
        shared
            .root()
            .join(shared.engine()?.config.effective_project(project)?.path)
    } else {
        shared.root().to_owned()
    };
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let data_root = shared.build_data_root()?;
    let (id, dir) = reserve(&data_root.join("build"))?;
    let socket_root = shared.private_chat_sockets();
    let launch = BuildLaunch {
        workspace: shared.root(),
        cwd: &cwd,
        data_root: &data_root,
        socket_root: socket_root.as_deref(),
        id: &id,
        resume: false,
        project: request.project.as_deref(),
        name: request.name.as_deref(),
        here: request.here,
        profile: request.profile.as_deref(),
        model: request.model.as_deref(),
        effort: request.effort.as_deref(),
        fallback: &request.fallback,
    };
    let (session, _) = match PiRunner::new(shared.chat_placement().as_ref()).start_build(launch) {
        Ok(started) => started,
        Err(error) => {
            let error = launch_error(&dir, 0, error);
            if let Err(cleanup) = discard_reservation(&dir) {
                return Err(
                    error.context(format!("discard unused Pi build reservation: {cleanup:#}"))
                );
            }
            return Err(error);
        }
    };
    shared.publish();
    Ok(require(shared, &id)?.row(shared, Some(&session)))
}

pub(crate) fn relaunch(shared: &Shared, id: &str) -> Result<SessionRelaunched> {
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let build = require(shared, id)?;
    if build.live(shared)?.is_none() {
        let data_root = shared.build_data_root()?;
        let socket_root = shared.private_chat_sockets();
        let checkout = Path::new(&build.record.repo_root);
        // A missing checkout needs the host's resume diagnosis, not a spawn error.
        let cwd = if checkout.is_dir() {
            checkout
        } else {
            shared.root()
        };
        let launch = BuildLaunch {
            workspace: shared.root(),
            cwd,
            data_root: &data_root,
            socket_root: socket_root.as_deref(),
            id,
            resume: true,
            project: None,
            name: None,
            here: false,
            profile: None,
            model: None,
            effort: None,
            fallback: &[],
        };
        let log = build.dir.join("session.log");
        let start = match fs::metadata(&log) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == ErrorKind::NotFound => 0,
            Err(err) => return Err(err).with_context(|| format!("inspect {}", log.display())),
        };
        PiRunner::new(shared.chat_placement().as_ref())
            .start_build(launch)
            .map_err(|err| launch_error(&build.dir, start, err))?;
        shared.publish();
    }
    Ok(SessionRelaunched { id: id.into() })
}

fn oversized_status(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PiRefusal>().is_some_and(|refusal| {
        refusal.command == "get_state" && refusal.code == "response_too_large"
    })
}

pub(crate) fn terminate(shared: &Shared, id: &str) -> Result<()> {
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let build = require(shared, id)?;
    if let Some((session, _)) = build.live(shared)? {
        let SessionRef::Pi { socket, inner, .. } = &session else {
            anyhow::bail!("build placement has no Pi socket");
        };
        // Never signal a terminal's process group. Its host owns terminal teardown.
        let terminal = match probe::<LegacyState>(socket, "get_state", json!({})) {
            Ok(state) => state.mode == Some(SessionMode::Interactive),
            // The old host responded, but may still own a terminal. Don't signal its group.
            Err(error) => oversized_status(&error),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let shutdown = forward::connect(socket, deadline).and_then(|stream| {
            Client::request_on::<_, Value>(stream, "shutdown", json!({}), deadline)
        });
        if terminal {
            shutdown.context("shut down terminal build host")?;
            loop {
                match probe::<Hello>(socket, "hello", json!({})) {
                    Ok(hello) if hello.session_id != id => break,
                    Err(error) if disconnected(&error) => break,
                    _ => {}
                }
                ensure!(Instant::now() < deadline, "Pi build did not shut down");
                thread::sleep(Duration::from_millis(25));
            }
        } else {
            let placement = shared.chat_placement();
            // Socket close precedes the host's exit hook. Give it time to save or
            // clean up the worktree before falling back to the detached runner.
            if shutdown.is_ok() {
                while Instant::now() < deadline {
                    if placement.status(inner)? == SessionStatus::Gone {
                        shared.publish();
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
            if build
                .dir
                .try_exists()
                .context("inspect stopped build directory")?
            {
                placement.stop(inner)?;
            }
        }
    }
    shared.publish();
    Ok(())
}

pub(crate) fn remove(shared: &Shared, request: BuildRemove) -> Result<BuildRemoved> {
    remove_with(shared, request, Command::output)
}

pub(crate) fn remove_with(
    shared: &Shared,
    request: BuildRemove,
    run: impl FnOnce(&mut Command) -> io::Result<Output>,
) -> Result<BuildRemoved> {
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ensure!(
        is_build(&request.id),
        "invalid: expected build id B-<base36>"
    );
    let data_root = shared.build_data_root()?;
    let build = require_at(shared, &data_root, &request.id)?;
    if build.live(shared)?.is_some() {
        return Err(refuse("stop the build session before removing it", vec![]));
    }

    let mut command = Command::new("corral-pi");
    command
        .args(["build", "--prune", "--session", &request.id, "--json"])
        .current_dir(shared.root())
        .env("CORRAL_NEXT_ROOT", shared.root());
    if request.dry_run {
        command.arg("--dry-run");
    }
    command.arg("--data-dir").arg(data_root);
    let output = run(&mut command).context("run corral-pi build prune")?;
    if !request.dry_run {
        // Prune can change the record even if it cannot report complete success.
        shared.publish();
    }

    let refusal = |message: String| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            refuse(message, vec![])
        } else {
            let stderr = &stderr[..stderr.floor_char_boundary(2048)];
            refuse(format!("{message}\n{stderr}"), vec![])
        }
    };
    if !output.status.success() {
        return Err(refusal(format!("Pi build prune failed: {}", output.status)));
    }
    let mut removed: BuildRemoved = serde_json::from_slice(&output.stdout)
        .map_err(|error| refusal(format!("invalid Pi build prune result: {error}")))?;
    removed.lines.truncate(16);
    for line in &mut removed.lines {
        line.truncate(line.floor_char_boundary(512));
    }
    ensure!(
        json_len(&removed).context("sizing Pi build prune result")? <= SESSION_PART_MAX,
        forward::PartTooLarge
    );
    Ok(removed)
}

fn reserve(root: &Path) -> Result<(String, PathBuf)> {
    fs::create_dir_all(root).with_context(|| format!("create build store {}", root.display()))?;
    const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let millis = jiff::Timestamp::now().as_millisecond().unsigned_abs();
    let mut time = [b'0'; 13];
    let mut index = time.len();
    let mut n = millis;
    while n > 0 {
        index -= 1;
        time[index] = BASE36[(n % 36) as usize];
        n /= 36;
    }
    let time =
        std::str::from_utf8(&time[index.min(time.len() - 1)..]).context("encode build id")?;
    loop {
        let mut random = [0_u8; 2];
        getrandom::fill(&mut random).map_err(|err| anyhow!("generate build id: {err}"))?;
        let suffix = u16::from_ne_bytes(random) as usize % (36 * 36);
        let id = format!(
            "B-{time}{}{}",
            char::from(BASE36[suffix / 36]),
            char::from(BASE36[suffix % 36])
        );
        let dir = root.join(&id);
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).with_context(|| format!("reserve {}", dir.display())),
        }
    }
}

fn launch_error(dir: &Path, offset: u64, error: anyhow::Error) -> anyhow::Error {
    let lines = (|| -> Result<Vec<String>> {
        let path = dir.join("session.log");
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let len = file.metadata().context("inspect Pi build log")?.len();
        file.seek(SeekFrom::Start(offset.max(len.saturating_sub(8192))))
            .context("seek Pi build log")?;
        let mut text = Vec::new();
        file.take(8192)
            .read_to_end(&mut text)
            .context("read Pi build log")?;
        Ok(String::from_utf8_lossy(&text)
            .lines()
            .rev()
            .filter(|line| !line.trim().is_empty())
            .take(2)
            .map(|line| bounded(line, 512))
            .collect())
    })();
    if let Ok(lines) = &lines
        && let [fix, what] = lines.as_slice()
    {
        return refuse(format!("{what}\n{fix}"), vec![]);
    }
    error
}

fn discard_reservation(dir: &Path) -> Result<()> {
    if dir
        .join("session.json")
        .try_exists()
        .context("inspect reserved build record")?
    {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect reservation {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry.context("read build reservation entry")?;
        if !matches!(
            entry.file_name().to_str(),
            Some("session.log" | "exit_code")
        ) {
            return Ok(());
        }
    }
    // Only retire the daemon's own staging artifacts, never transcripts or host data.
    for file in ["session.log", "exit_code"] {
        let path = dir.join(file);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("remove staged {}", path.display()));
            }
        }
    }
    match fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(err) => {
            Err(err).with_context(|| format!("remove unused reservation {}", dir.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_build_title_priority_and_empty_values() {
        let record: Record = serde_json::from_value(json!({
            "id": "build-1", "name": "fallback", "title": "explicit",
            "repoRoot": "/repo", "worktree": "/worktree", "here": false,
            "created": "now", "updated": "now"
        }))
        .unwrap();
        let journal: Journal = serde_json::from_value(json!({"title": "first prompt"})).unwrap();
        assert_eq!(
            build_title(
                record.title.as_deref(),
                journal.title.as_deref(),
                &record.name
            ),
            "explicit"
        );
        assert_eq!(
            build_title(None, journal.title.as_deref(), &record.name),
            "first prompt"
        );
        assert_eq!(
            build_title(Some("  "), journal.title.as_deref(), &record.name),
            "first prompt"
        );
        assert_eq!(build_title(Some(""), Some(" \n"), &record.name), "fallback");
        assert_eq!(build_title(None, None, &record.name), "fallback");
    }
}

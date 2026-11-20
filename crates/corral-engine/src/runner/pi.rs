//! Pi host placement and its local, newline-delimited session protocol.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::{
    ffi::OsStr,
    fs::{self, DirBuilder},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::{SocketAddr, UnixStream},
    },
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
#[cfg(unix)]
use sha2::{Digest, Sha256};

use super::{InputTag, LaunchSpec, Origin, Runner, SessionRef, SessionStatus};
use crate::{config::shell_quote, refuse};

mod build;
pub use build::{BuildLaunch, build_session, build_socket, is_build_id, pi_data_root};

mod chat;
pub use chat::{ChatLaunch, chat_session};

const VERSION: u32 = 2;
pub(super) const FRAME_MAX: usize = 64 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
static REQUEST_ID: AtomicU64 = AtomicU64::new(0);

/// Each request uses its own connection. IDs also correlate delayed settlements
/// from multiple clients, and are never used for deduplication or authorization.
#[derive(Clone, Copy, Debug)]
pub struct Client<'a> {
    socket: &'a Path,
    dir: Option<&'a Path>,
}

#[derive(Debug, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub role: String,
    pub session_id: String,
    pub session_file: String,
    pub pid: u32,
}

#[derive(Debug, Deserialize)]
pub struct SubmitReply {
    pub request_id: String,
    pub input: Option<String>,
    pub disposition: Disposition,
    pub turn: Option<u64>,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Started,
    Queued,
    Handled,
    Pending,
    Rejected,
}

#[derive(Serialize)]
struct Submit<'a> {
    text: &'a str,
    origin: Origin,
    busy_policy: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    typed_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<&'a str>,
    #[serde(skip_serializing_if = "<[Value]>::is_empty")]
    attachments: &'a [Value],
}

impl<'a> Client<'a> {
    pub fn new(socket: &'a Path) -> Self {
        Self { socket, dir: None }
    }

    pub fn with_dir(socket: &'a Path, dir: &'a Path) -> Self {
        Self {
            socket,
            dir: Some(dir),
        }
    }

    pub fn for_session(session: &'a SessionRef) -> Result<Self> {
        match session {
            SessionRef::Pi { socket, dir, .. } => Ok(Self::with_dir(socket, dir)),
            _ => bail!("session does not have a Pi socket"),
        }
    }

    /// Send a v2 command and decode its result. The daemon can use this for host
    /// commands that do not implement an engine operation, such as `live_since`.
    pub fn request<T: Serialize, R: DeserializeOwned>(&self, command: &str, args: T) -> Result<R> {
        self.request_with_timeout(command, args, IO_TIMEOUT)
    }

    pub fn request_with_timeout<T: Serialize, R: DeserializeOwned>(
        &self,
        command: &str,
        args: T,
        timeout: Duration,
    ) -> Result<R> {
        let id = request_id();
        self.exchange(command, &id, &encode(command, &id, args)?, timeout)
    }

    /// Exchange a v2 request over a caller-connected blocking stream by an
    /// absolute deadline, including any time the caller spent connecting.
    #[cfg(unix)]
    pub fn request_on<T: Serialize, R: DeserializeOwned>(
        stream: UnixStream,
        command: &str,
        args: T,
        deadline: Instant,
    ) -> Result<R> {
        let id = request_id();
        Self::exchange_on(stream, command, &id, &encode(command, &id, args)?, deadline)
    }

    pub fn hello(&self) -> Result<Hello> {
        self.hello_with_timeout(IO_TIMEOUT)
    }

    pub fn hello_with_timeout(&self, timeout: Duration) -> Result<Hello> {
        let id = request_id();
        let result: Value =
            self.exchange("hello", &id, &encode("hello", &id, json!({}))?, timeout)?;
        let protocol = result["protocol"]
            .as_u64()
            .context("Pi hello did not report a protocol number; reinstall corral-pi")?;
        check_version(protocol)?;
        serde_json::from_value(result).context("decode Pi hello result")
    }

    pub fn submit_input(&self, text: &str, origin: Origin) -> Result<SubmitReply> {
        self.submit(text, origin, None)
    }

    pub fn submit_input_tagged(
        &self,
        text: &str,
        origin: Origin,
        tag: InputTag<'_>,
    ) -> Result<SubmitReply> {
        let reply = self.submit(text, origin, Some(tag))?;
        if reply.disposition == Disposition::Rejected {
            return Err(refuse(
                reply.reason.unwrap_or_else(|| "Pi rejected input".into()),
                Vec::new(),
            ));
        }
        if reply.input.as_deref() != Some(tag.input_id) {
            return Err(refuse(
                "the Pi host did not acknowledge this input id; stop it and resume it on the current corral-pi",
                Vec::new(),
            ));
        }
        Ok(reply)
    }

    fn submit(&self, text: &str, origin: Origin, tag: Option<InputTag<'_>>) -> Result<SubmitReply> {
        const COMMAND: &str = "submit_input";
        let id = request_id();
        let mut frame = if text.len() < FRAME_MAX {
            encode_body(COMMAND, &id, submit(text, origin, tag))?
        } else {
            Vec::new()
        };
        if frame.is_empty() || frame.len() > FRAME_MAX {
            let dir = self
                .dir
                .context("oversized Pi input needs a session directory")?;
            let message = spill(dir, text)?;
            frame = encode(COMMAND, &id, submit(&message, origin, tag))?;
        } else {
            frame.push(b'\n');
        }
        self.exchange(COMMAND, &id, &frame, IO_TIMEOUT)
    }

    pub fn cancel(&self) -> Result<Value> {
        self.request("cancel", json!({}))
    }

    pub fn pause(&self) -> Result<Value> {
        self.request("pause", json!({}))
    }

    pub fn resume(&self) -> Result<Value> {
        self.request("resume", json!({}))
    }

    #[cfg(unix)]
    fn exchange<R: DeserializeOwned>(
        &self,
        command: &str,
        id: &str,
        frame: &[u8],
        timeout: Duration,
    ) -> Result<R> {
        Self::exchange_on(
            self.connect()?,
            command,
            id,
            frame,
            Instant::now() + timeout,
        )
    }

    #[cfg(unix)]
    fn exchange_on<R: DeserializeOwned>(
        mut stream: UnixStream,
        command: &str,
        id: &str,
        frame: &[u8],
        deadline: Instant,
    ) -> Result<R> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "timed out writing Pi request");
        stream
            .set_write_timeout(Some(remaining))
            .context("set Pi socket write timeout")?;
        stream
            .write_all(frame)
            .with_context(|| format!("write Pi {command} request"))?;
        let frame = read_frame(&mut stream, deadline)
            .with_context(|| format!("read Pi {command} reply"))?;
        let reply: Reply<R> =
            serde_json::from_slice(&frame).with_context(|| format!("decode Pi {command} reply"))?;
        check_version(reply.version)?;
        ensure!(
            reply.id.as_deref() == Some(id),
            "unexpected Pi {command} reply ID {:?}",
            reply.id
        );
        if !reply.ok {
            let error: ReplyError =
                serde_json::from_value(reply.error.context("missing Pi reply error")?)
                    .context("decode Pi reply error")?;
            return Err(PiRefusal {
                command: command.into(),
                code: error.code,
                reason: error.reason,
            }
            .into());
        }
        reply
            .result
            .with_context(|| format!("Pi {command} reply is missing its result"))
    }

    #[cfg(not(unix))]
    fn exchange<R: DeserializeOwned>(&self, _: &str, _: &str, _: &[u8], _: Duration) -> Result<R> {
        bail!(
            "Pi sessions require Unix sockets ({})",
            self.socket.display()
        )
    }

    #[cfg(unix)]
    fn connect(&self) -> Result<UnixStream> {
        UnixStream::connect(self.socket)
            .with_context(|| format!("connect to Pi session socket {}", self.socket.display()))
    }
}

#[cfg(unix)]
fn read_frame(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>> {
    let mut frame = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "timed out waiting for Pi reply");
        stream
            .set_read_timeout(Some(remaining))
            .context("set Pi socket read timeout")?;
        let count = match stream.read(&mut buf) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            read => read.context("read from Pi socket")?,
        };
        if count == 0 && frame.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Pi socket closed").into());
        }
        ensure!(count != 0, "incomplete Pi reply");
        let chunk = &buf[..count];
        let end = chunk.iter().position(|&byte| byte == b'\n');
        let part = &chunk[..end.unwrap_or(count)];
        ensure!(
            frame.len() + part.len() <= FRAME_MAX,
            "Pi reply exceeds 64 KiB"
        );
        frame.extend_from_slice(part);
        if end.is_some() {
            return Ok(frame);
        }
    }
}

#[cfg(unix)]
#[derive(Deserialize)]
struct Reply<R> {
    version: u64,
    id: Option<String>,
    ok: bool,
    result: Option<R>,
    error: Option<Value>,
}

#[cfg(unix)]
#[derive(Deserialize)]
struct ReplyError {
    code: String,
    reason: String,
}

#[derive(Debug)]
pub struct PiRefusal {
    pub command: String,
    pub code: String,
    pub reason: String,
}

impl std::fmt::Display for PiRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Pi {} refused ({}): {}",
            self.command, self.code, self.reason
        )
    }
}

impl std::error::Error for PiRefusal {}

fn request_id() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let count = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:x}-{:x}-{count:x}",
        std::process::id(),
        elapsed.as_nanos()
    )
}

fn check_version(version: u64) -> Result<()> {
    ensure!(
        version == u64::from(VERSION),
        "Pi protocol {version} is not supported (expected {VERSION}); reinstall corral-pi"
    );
    Ok(())
}

fn submit<'a>(text: &'a str, origin: Origin, tag: Option<InputTag<'a>>) -> Submit<'a> {
    Submit {
        text,
        origin,
        busy_policy: if tag.is_some_and(|tag| tag.follow_up) {
            "follow_up"
        } else {
            "steer"
        },
        input_id: tag.map(|tag| tag.input_id),
        to: tag.and_then(|tag| tag.to),
        typed_at: tag.and_then(|tag| tag.typed_at),
        device: tag.and_then(|tag| tag.device),
        attachments: tag.map_or(&[], |tag| tag.attachments),
    }
}

#[derive(Serialize)]
struct Request<'a, T> {
    version: u32,
    id: &'a str,
    command: &'a str,
    args: T,
}

fn encode<T: Serialize>(command: &str, id: &str, args: T) -> Result<Vec<u8>> {
    let mut frame = encode_body(command, id, args)?;
    ensure!(
        frame.len() <= FRAME_MAX,
        "Pi {command} request exceeds 64 KiB"
    );
    frame.push(b'\n');
    Ok(frame)
}

fn encode_body<T: Serialize>(command: &str, id: &str, args: T) -> Result<Vec<u8>> {
    serde_json::to_vec(&Request {
        version: VERSION,
        id,
        command,
        args,
    })
    .with_context(|| format!("encode Pi {command} request"))
}

fn unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            io::ErrorKind::NotFound
                | io::ErrorKind::ConnectionRefused
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::UnexpectedEof
        )
    })
}

#[derive(Deserialize)]
struct State {
    paused: bool,
    turn: Turn,
    owner_questions: Vec<Question>,
    last_error: Option<String>,
}

#[derive(Deserialize)]
struct Turn {
    open: bool,
}

#[derive(Deserialize)]
struct Question {
    question: String,
}

/// Launches Pi through the selected pane or process runner. Existing non-Pi
/// session references are passed straight to that runner unchanged.
pub struct PiRunner<'a> {
    inner: &'a dyn Runner,
    #[cfg(test)]
    socket_dir: Option<&'a Path>,
}

impl<'a> PiRunner<'a> {
    pub fn new(inner: &'a dyn Runner) -> Self {
        Self {
            inner,
            #[cfg(test)]
            socket_dir: None,
        }
    }

    #[cfg(test)]
    pub(super) fn in_test_socket_dir(mut self, dir: &'a Path) -> Self {
        self.socket_dir = Some(dir);
        self
    }

    /// Read status and the pause flag from the same observation of the Pi host.
    pub fn observe(&self, session: &SessionRef) -> Result<(SessionStatus, Option<bool>)> {
        let SessionRef::Pi { socket, inner, .. } = session else {
            return Ok((self.inner.status(session)?, None));
        };
        let state: State = match Client::new(socket).request("get_state", json!({})) {
            Ok(state) => state,
            Err(error) if unavailable(&error) => return Ok((self.inner.status(inner)?, None)),
            Err(error) => return Err(error),
        };
        let status = if let Some(question) = state.owner_questions.into_iter().next() {
            SessionStatus::Blocked {
                excerpt: question.question,
            }
        } else if state.turn.open {
            SessionStatus::Working
        } else if !state.paused
            && let Some(excerpt) = state.last_error
        {
            SessionStatus::Blocked { excerpt }
        } else {
            SessionStatus::Idle
        };
        Ok((status, Some(state.paused)))
    }

    /// An older Pi host can answer but fail to fit its diagnostic state in a frame.
    /// Its refusal proves liveness, not a lifecycle status. Never use this in reconciliation.
    pub(crate) fn is_live(&self, session: &SessionRef) -> Result<bool> {
        match self.status(session) {
            Ok(status) => Ok(status != SessionStatus::Gone),
            Err(error)
                if matches!(session, SessionRef::Pi { .. })
                    && error.downcast_ref::<PiRefusal>().is_some_and(|refusal| {
                        refusal.command == "get_state" && refusal.code == "response_too_large"
                    }) =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    fn wait_for_hello(&self, client: &Client<'_>, inner: &SessionRef) -> Result<Hello> {
        let deadline = Instant::now() + START_TIMEOUT;
        let mut check_placement = Instant::now();
        loop {
            let now = Instant::now();
            let remaining = deadline.saturating_duration_since(now);
            ensure!(
                !remaining.is_zero(),
                "timed out waiting for corral-pi hello"
            );
            match client.hello_with_timeout(remaining) {
                Ok(hello) => return Ok(hello),
                Err(error)
                    if unavailable(&error)
                        || error
                            .downcast_ref::<PiRefusal>()
                            .is_some_and(|error| error.code == "starting") => {}
                Err(error) => return Err(error),
            }
            if now >= check_placement {
                ensure!(
                    self.inner.status(inner)? != SessionStatus::Gone,
                    "corral-pi exited before hello; reinstall corral-pi or check the session log"
                );
                check_placement = now + Duration::from_secs(1);
            }
            thread::sleep(POLL_INTERVAL.min(remaining));
        }
    }

    fn socket(&self, dir: &Path) -> Result<PathBuf> {
        #[cfg(test)]
        if let Some(root) = self.socket_dir {
            return session_socket(root, dir);
        }
        session_socket(&runtime_dir(), dir)
    }
}

impl Runner for PiRunner<'_> {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef> {
        let reviewer = spec
            .env
            .iter()
            .any(|(key, value)| key == "CORRAL_ACTOR" && value == "reviewer");
        let dir = if reviewer {
            spec.dir
                .parent()
                .context("reviewer run has no task directory")?
                .join("reviewer")
        } else {
            spec.dir.clone()
        };
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create Pi session parent {}", parent.display()))?;
        }
        private_dir(&dir, true)?;
        let dir = dir.canonicalize().context("resolve Pi session directory")?;
        let socket = self.socket(&dir)?;
        let client = Client::new(&socket);
        match client.hello() {
            Ok(_) => bail!(
                "Pi session already live; corral stop {}",
                spec.tokens
                    .iter()
                    .find(|(key, _)| key == "task")
                    .map_or("<id>", |(_, value)| value)
            ),
            Err(error) if unavailable(&error) => {}
            Err(error) => return Err(error),
        }

        let mut launch = spec.clone();
        launch.command.push_str(" --dir ");
        launch.command.push_str(&quoted_path(&dir)?);
        launch.command.push_str(" --socket ");
        launch.command.push_str(&quoted_path(&socket)?);
        let inner = self.inner.start(&launch)?;
        if let Err(error) = self.wait_for_hello(&client, &inner) {
            return match self.inner.stop(&inner) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("failed to stop Pi launch placement: {cleanup:#}")))
                }
            };
        }
        Ok(SessionRef::Pi {
            socket,
            inner: Box::new(inner),
            dir,
        })
    }

    fn status(&self, session: &SessionRef) -> Result<SessionStatus> {
        self.observe(session).map(|(status, _)| status)
    }

    fn send(&self, session: &SessionRef, text: &str, origin: Origin) -> Result<()> {
        let SessionRef::Pi { socket, dir, .. } = session else {
            return self.inner.send(session, text, origin);
        };
        let reply = Client::with_dir(socket, dir).submit_input(text, origin).map_err(|error| {
            if unavailable(&error) {
                refuse(format!("Pi session socket unavailable: {error:#}; use `corral retry <task>` or `corral review <task>`"), Vec::new())
            } else {
                error
            }
        })?;
        ensure!(
            reply.disposition != Disposition::Rejected,
            "Pi input rejected: {}",
            reply.reason.as_deref().unwrap_or("no reason given")
        );
        Ok(())
    }

    fn send_tagged(
        &self,
        session: &SessionRef,
        text: &str,
        origin: Origin,
        tag: InputTag<'_>,
    ) -> Result<SubmitReply> {
        let SessionRef::Pi { socket, dir, .. } = session else {
            return self.inner.send_tagged(session, text, origin, tag);
        };
        Client::with_dir(socket, dir)
            .submit_input_tagged(text, origin, tag)
            .map_err(|error| {
                if unavailable(&error) {
                    refuse(
                        "session has no running Pi host; relaunch it first",
                        Vec::new(),
                    )
                } else {
                    error
                }
            })
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        let SessionRef::Pi { socket, inner, .. } = session else {
            return self.inner.stop(session);
        };
        #[cfg(unix)]
        {
            let deadline = Instant::now() + IO_TIMEOUT;
            let client = Client::new(socket);
            // An unreachable or uncooperative host must not prevent closing its placement.
            if client.request::<_, Value>("shutdown", json!({})).is_ok() {
                while Instant::now() < deadline && client.connect().is_ok() {
                    thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
        #[cfg(not(unix))]
        let _ = socket;
        self.inner.stop(inner)
    }

    fn focus(&self, session: &SessionRef) -> Result<()> {
        if let SessionRef::Pi { inner, .. } = session {
            self.inner.focus(inner)
        } else {
            self.inner.focus(session)
        }
    }
}

pub(super) fn quoted_path(path: &Path) -> Result<String> {
    shell_quote(
        path.to_str()
            .with_context(|| format!("Pi path is not UTF-8: {}", path.display()))?,
    )
}

fn spill(dir: &Path, text: &str) -> Result<String> {
    let mut file = tempfile::Builder::new()
        .prefix("input-")
        .suffix(".md")
        .tempfile_in(dir)
        .with_context(|| format!("create Pi input file in {}", dir.display()))?;
    file.write_all(text.as_bytes())
        .context("write oversized Pi input")?;
    // The host may retain input in its queue across restarts, so keep the file.
    let (_, path) = file.keep().context("keep oversized Pi input file")?;
    Ok(format!("Read {} for the full message.", path.display()))
}

#[cfg(unix)]
fn uid() -> u32 {
    // SAFETY: geteuid takes no arguments and has no failure case.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Path::new(&dir).join("corral-next"),
        None => Path::new("/tmp").join(format!("corral-next-{}", uid())),
    }
}

#[cfg(not(unix))]
fn runtime_dir() -> PathBuf {
    PathBuf::new()
}

#[cfg(unix)]
fn session_socket(root: &Path, dir: &Path) -> Result<PathBuf> {
    private_dir(root, true)?;
    let root = root.canonicalize().context("resolve Pi socket directory")?;
    let digest = Sha256::digest(OsStr::as_bytes(dir.as_os_str()));
    let prefix = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .context("read Pi socket digest prefix")?,
    );
    let socket = root.join(format!("{prefix:016x}.sock"));
    SocketAddr::from_pathname(&socket)
        .with_context(|| format!("build Pi socket address {}", socket.display()))?;
    Ok(socket)
}

#[cfg(not(unix))]
fn session_socket(_: &Path, _: &Path) -> Result<PathBuf> {
    bail!("Pi sessions require Unix sockets")
}

#[cfg(unix)]
pub(crate) fn private_dir(dir: &Path, reuse: bool) -> Result<()> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && reuse => {}
        Err(error) => {
            return Err(error).with_context(|| format!("create Pi directory {}", dir.display()));
        }
    }
    let meta = fs::symlink_metadata(dir)
        .with_context(|| format!("inspect Pi directory {}", dir.display()))?;
    ensure!(
        meta.is_dir() && meta.permissions().mode() & 0o777 == 0o700 && meta.uid() == uid(),
        "Pi directory {} must be owned by this user and have mode 0700",
        dir.display()
    );
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn private_dir(_: &Path, _: bool) -> Result<()> {
    bail!("Pi sessions require Unix sockets")
}

//! The artifact server: one loopback-only, read-only HTTP server per workspace that serves HTML and Markdown
//! artifacts live (`corral artifact serve`, and ⏎ / click on an HTML artifact in the board).
//!
//! It binds 127.0.0.1 on a random port, puts a random token as the first path segment of every URL and
//! answers only from recorded snapshots under `.corral/artifacts` (see `pages`). Its address lives in
//! the versioned `artifact-server` record (in the owner-only database directory);
//! [`ensure_server`] reuses a live server or starts one as a
//! detached `corral artifact serve --daemon`. It exits after [`DEFAULT_IDLE`] without requests or open pages,
//! on `/<token>/shutdown` ([`stop_server`]) and when the board that used it exits.

mod http;
mod pages;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::Artifact;
use corral_core::records::RecordArea;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::viewers::{Host, SystemHost, UrlRoute};

pub use pages::artifact_url_path;

/// How long the server waits without requests or open pages before it exits.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(15 * 60);

const STATE: RecordArea<ServerInfo> = RecordArea::new("artifact-server");
const LIVE_KEY: &str = "live";
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

static USED: AtomicBool = AtomicBool::new(false);

/// A running server's address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

impl ServerInfo {
    /// `http://127.0.0.1:<port>/<token>/`.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/", self.port, self.token)
    }
}

/// A served page and what happened when corral tried to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub url: String,
    pub port: u16,
    /// The browser launch, when one ran.
    pub launched: Option<Vec<String>>,
    /// Why no browser was launched, with a port-forward hint over SSH.
    pub note: Option<String>,
}

pub(crate) struct Shared {
    ctx: Ctx,
    info: ServerInfo,
    started: Instant,
    last_ms: AtomicU64,
    streams: AtomicUsize,
    stop: AtomicBool,
}

impl Shared {
    fn touch(&self) {
        let ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_ms.store(ms, Ordering::SeqCst);
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn idle_for(&self) -> Duration {
        self.started
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::SeqCst)))
    }

    /// Stops accepting: the flag ends event streams, the self-connection wakes the blocked accept.
    fn shut_down(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.info.port));
        // Only wakes `accept`; when it fails the listener is already gone.
        let _ = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT);
    }
}

fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("reading artifact server token entropy: {error}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn write_state(ctx: &Ctx, info: &ServerInfo) -> Result<()> {
    anyhow::ensure!(
        !ctx.store.is_snapshot(),
        "a snapshot cannot register a server"
    );
    // The token used to live in a 0600 file. Protect the database and its WAL/SHM
    // together before persisting it; newly created SQLite sidecars inherit this boundary.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = ctx.store.dir().join("db");
        let metadata = std::fs::symlink_metadata(&dir)
            .with_context(|| format!("reading {}", dir.display()))?;
        anyhow::ensure!(
            metadata.is_dir(),
            "database directory must not be a symlink"
        );
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("protecting {}", dir.display()))?;
    }
    STATE.save_intent(&ctx.store, LIVE_KEY, info)
}

fn read_state(ctx: &Ctx) -> Result<Option<ServerInfo>> {
    STATE.load(&ctx.store, LIVE_KEY)
}

fn clear_state(ctx: &Ctx, info: &ServerInfo) -> Result<()> {
    if read_state(ctx)?.as_ref() == Some(info) {
        STATE.delete(&ctx.store, LIVE_KEY)?;
    }
    Ok(())
}

/// GET `/<token>/<path>` on a server; the response body when it answers 200.
fn request(info: &ServerInfo, path: &str) -> Result<String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let mut s = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)
        .with_context(|| format!("connecting to the artifact server on port {}", info.port))?;
    s.set_read_timeout(Some(PROBE_TIMEOUT))?;
    s.set_write_timeout(Some(PROBE_TIMEOUT))?;
    write!(
        s,
        "GET /{}/{path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        info.token, info.port
    )?;
    let mut out = String::new();
    s.read_to_string(&mut out)
        .context("reading the artifact server response")?;
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    if !head.starts_with("HTTP/1.1 200") {
        bail!(
            "artifact server answered {}",
            head.lines().next().unwrap_or("nothing")
        );
    }
    Ok(body.to_string())
}

/// The workspace's live server, if its canonical registration answers for this `.corral/`.
pub fn running_server(ctx: &Ctx) -> Option<ServerInfo> {
    let info = read_state(ctx).ok()??;
    probe(ctx, info)
}

fn probe(ctx: &Ctx, info: ServerInfo) -> Option<ServerInfo> {
    let body = request(&info, "health").ok()?;
    let mut lines = body.lines();
    (lines.next() == Some("corral-artifact-server")
        && lines.next() == Some(&ctx.store.dir().display().to_string()))
    .then_some(info)
}

/// True once this process started or reused a server (the board stops it on exit).
pub fn artifact_server_used() -> bool {
    USED.load(Ordering::SeqCst)
}

/// Reuses the live server or starts `corral --root <root> artifact serve --daemon` detached and reads its
/// address from the first line it prints. Only the corral binary can start one.
pub fn ensure_server(ctx: &Ctx) -> Result<ServerInfo> {
    anyhow::ensure!(!ctx.store.is_snapshot(), "a snapshot cannot start a server");
    let dir = ctx.store.dir().join("lock");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock_path = dir.join("artifact-server.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    lock.lock()
        .with_context(|| format!("locking {}", lock_path.display()))?;
    // Corrupt or future-version registrations are not permission to start another daemon.
    if let Some(info) = read_state(ctx)?.and_then(|info| probe(ctx, info)) {
        USED.store(true, Ordering::SeqCst);
        return Ok(info);
    }
    let exe = std::env::current_exe().context("locating the corral binary")?;
    if exe.file_stem().is_none_or(|s| s != "corral") {
        bail!(
            "the artifact server runs from the corral binary, not {}",
            exe.display()
        );
    }
    let mut cmd = Command::new(&exe);
    cmd.arg("--root")
        .arg(ctx.store.root())
        .args(["artifact", "serve", "--daemon"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting {} artifact serve --daemon", exe.display()))?;
    let stdout = child
        .stdout
        .take()
        .context("artifact server has no stdout")?;
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .context("reading the artifact server address")?;
    let info: ServerInfo = serde_json::from_str(line.trim())
        .with_context(|| format!("artifact server did not start (printed {:?})", line.trim()))?;
    std::thread::spawn(move || child.wait());
    USED.store(true, Ordering::SeqCst);
    Ok(info)
}

/// Asks the live server to exit; false when none was running.
pub fn stop_server(ctx: &Ctx) -> Result<bool> {
    let Some(info) = running_server(ctx) else {
        return Ok(false);
    };
    request(&info, "shutdown")?;
    Ok(true)
}

/// Runs a server in this process until it is idle for `idle`, shut down or the listener fails. `ready` gets
/// the address once the listener is bound and the canonical registration committed.
pub fn run_server(
    ctx: Ctx,
    idle: Duration,
    ready: impl FnOnce(&ServerInfo) -> Result<()>,
) -> Result<()> {
    anyhow::ensure!(!ctx.store.is_snapshot(), "a snapshot cannot run a server");
    // Separate from the launcher lease: ensure_server holds that one while it
    // waits for this process's ready line. This lease survives for the daemon's
    // lifetime and is released by the OS after a crash.
    let _live = ctx
        .store
        .try_lock("artifact-server-live")?
        .context("an artifact server already holds the workspace lease")?;
    read_state(&ctx)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("binding 127.0.0.1")?;
    let port = listener
        .local_addr()
        .context("reading the bound port")?
        .port();
    let info = ServerInfo {
        port,
        token: new_token()?,
        pid: std::process::id(),
    };
    write_state(&ctx, &info)?;
    if let Err(error) = ready(&info) {
        clear_state(&ctx, &info)?;
        return Err(error);
    }
    let shared = Arc::new(Shared {
        ctx,
        info,
        started: Instant::now(),
        last_ms: AtomicU64::new(0),
        streams: AtomicUsize::new(0),
        stop: AtomicBool::new(false),
    });
    let watchdog = Arc::clone(&shared);
    let tick = (idle / 4).clamp(Duration::from_millis(20), Duration::from_secs(1));
    std::thread::spawn(move || {
        while !watchdog.stopping() {
            std::thread::sleep(tick);
            if watchdog.streams.load(Ordering::SeqCst) == 0 && watchdog.idle_for() >= idle {
                watchdog.shut_down();
            }
        }
    });
    for conn in listener.incoming() {
        if shared.stopping() {
            break;
        }
        // A failed accept (client reset, fd pressure) affects one connection, not the server.
        let Ok(stream) = conn else { continue };
        shared.touch();
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            // Per-connection errors are a client that went away mid-request; nothing to report.
            let _ = http::handle(&shared, stream);
        });
    }
    clear_state(&shared.ctx, &shared.info)?;
    Ok(())
}

/// Serves `url_path` (below the token) and shows it: the browser route from [`crate::viewers::url_route`], or
/// only the URL (with why) when `open` is false or no browser can be reached.
pub fn serve_path(ctx: &Ctx, url_path: &str, open: bool) -> Result<Served> {
    let info = ensure_server(ctx)?;
    let url = format!("{}{url_path}", info.base_url());
    let mut served = Served {
        url: url.clone(),
        port: info.port,
        launched: None,
        note: None,
    };
    if !open {
        served.note = Some("not opened (--no-open)".into());
        return Ok(served);
    }
    let host = SystemHost;
    match crate::viewers::url_route(&ctx.store.config.viewers, &url, info.port, &host) {
        UrlRoute::Launch(argv) => match host.launch(&argv, argv[0] != "explorer.exe", false) {
            Ok(()) => served.launched = Some(argv),
            Err(e) => served.note = Some(format!("could not open a browser: {e:#}")),
        },
        UrlRoute::Print(why) => served.note = Some(why),
    }
    Ok(served)
}

/// Serves one artifact (see [`serve_path`]).
pub fn serve_artifact(ctx: &Ctx, a: &Artifact, open: bool) -> Result<Served> {
    let path = artifact_url_path(a).context("artifact path has no task directory")?;
    serve_path(ctx, &path, open)
}

/// Serves the document index of one task, or of the workspace.
pub fn serve_index(ctx: &Ctx, task: Option<&str>, open: bool) -> Result<Served> {
    let path = match task {
        Some(t) => format!(
            "t/{}/",
            pages::encode(&corral_core::Store::normalize_id(t)?)
        ),
        None => String::new(),
    };
    serve_path(ctx, &path, open)
}

#[cfg(test)]
#[path = "artifact_server_tests.rs"]
mod tests;

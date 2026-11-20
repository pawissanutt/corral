//! Local owner RPC server for the simple engine.
#![cfg(unix)]

mod app;
mod assistant;
mod builds;
mod config;
mod conn;
mod devices;
mod health;
mod identity;
mod ops;
mod pairing;
mod server;
mod sessions;
mod state;
mod topics;
mod views;

pub mod client;

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::ErrorKind,
    net::{SocketAddr, TcpListener},
    os::unix::{
        fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use corral_engine::{Config, Store};
use corral_protocol::device;

use crate::{
    devices::Devices,
    state::{RunnerFactory, Shared},
};

const STATE_DIR: &str = ".corral-next";
const SYNC_INTERVAL: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct RunOptions {
    pub listen: Option<SocketAddr>,
    pub web_origins: Vec<String>,
    pub web_dir: Option<PathBuf>,
    pub web_endpoint: Option<String>,
    pub push_key: Option<PathBuf>,
    pub push_subject: Option<String>,
    pub push_origins: Vec<String>,
}

pub fn socket_path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join("daemon.sock")
}

pub fn run(root: &Path, options: RunOptions) -> Result<()> {
    run_with_runner(root, options, None)
}

fn run_with_runner(root: &Path, options: RunOptions, runner: Option<RunnerFactory>) -> Result<()> {
    let root = root.canonicalize().context("resolve daemon workspace")?;
    let state = root.join(STATE_DIR);
    let db = state.join("corral.db");
    ensure!(
        db.is_file(),
        "workspace is not initialized; run corral-next init"
    );
    let config = Config::load(&state.join("corral.toml"))?
        .daemon
        .unwrap_or_default();
    let listen = options.listen.or(config.listen);
    let origins = if options.web_origins.is_empty() {
        config.web_origin
    } else {
        options.web_origins
    };
    let web_dir = options.web_dir.or(config.web_dir);
    if let Some(path) = &web_dir {
        ensure!(!path.as_os_str().is_empty(), "web_dir must not be empty");
        let address =
            listen.context("web_dir requires an app listener ([daemon] listen or --listen)")?;
        let ip = address.ip().to_canonical();
        ensure!(
            !ip.is_unspecified() && !ip.is_multicast(),
            "web_dir requires a concrete unicast app listen address"
        );
    }
    let web_dir = web_dir.map(|path| root.join(path));
    let web_endpoint = options.web_endpoint.or(config.web_endpoint);
    if let Some(endpoint) = &web_endpoint {
        ensure!(device::valid_endpoint(endpoint), "invalid web_endpoint");
    }
    let path = socket_path(&root);
    let Some((_lock, socket, listener)) = bind_socket(path)? else {
        return Ok(());
    };
    let mut devices = Devices::open(&state)?;
    devices.configure_push(
        options.push_key.as_deref(),
        options.push_subject.as_deref().unwrap_or(""),
        &options.push_origins,
    )?;
    let shared = Arc::new(Shared::with_devices(root, runner, devices)?);
    let store = Store::open(&db)?;
    let seq = store.latest_seq()?;
    let version = store.data_version()?;
    shared.latest_seq.store(seq, Ordering::Release);
    let app_listener = listen
        .map(|address| {
            TcpListener::bind(address).with_context(|| format!("bind app listener {address}"))
        })
        .transpose()?;
    let started = (|| -> Result<_> {
        topics::start_session_live(&shared)?;
        if let Some(listener) = app_listener {
            app::serve_listener(
                listener,
                Arc::clone(&shared),
                origins,
                web_endpoint.as_deref(),
                web_dir,
            )?;
        }
        server::start(listener, Arc::clone(&shared))?;
        let sync = Arc::clone(&shared);
        thread::Builder::new()
            .name("corrald-sync".into())
            .spawn(move || sync_loop(&sync))
            .context("start daemon sync loop")
    })();
    let result = match started {
        Ok(sync) => {
            poll_loop(&shared, &store, seq, version);
            shared.stop();
            sync.join()
                .map_err(|_| anyhow::anyhow!("sync loop panicked"))
        }
        Err(error) => {
            shared.stop();
            Err(error)
        }
    };
    shared.wait_ops();
    shared.close();
    topics::join_session_live(&shared);
    shared.wait_workers(Duration::from_secs(3));
    drop(socket);
    result?;
    if let Some(error) = shared.lock().server_error.take() {
        anyhow::bail!("daemon server failed: {error}");
    }
    Ok(())
}

fn sync_loop(shared: &Shared) {
    while shared.begin_sync(SYNC_INTERVAL) {
        match shared.engine() {
            Ok(engine) => {
                let reachable = shared.runner_reachable(&engine);
                let error = engine.sync().err().map(|error| format!("{error:#}"));
                shared.finish_sync(reachable, error);
            }
            Err(error) => shared.finish_sync(false, Some(format!("{error:#}"))),
        }
    }
}

fn poll_loop(shared: &Shared, store: &Store, mut seq: u64, mut version: u64) {
    while !shared.lock().stop {
        shared.wait(POLL_INTERVAL);
        if shared.lock().stop {
            break;
        }
        if let Err(error) = shared.refresh_config() {
            eprintln!("corrald configuration poll: {error:#}");
        }
        let result = (|| -> Result<()> {
            // Sample data_version first so a non-event commit racing this scan is seen next time.
            let observed_version = store.data_version()?;
            let head = store.latest_seq()?;
            if head != seq || observed_version != version {
                shared.latest_seq.fetch_max(head, Ordering::AcqRel);
                shared.publish();
                version = observed_version;
            }
            while seq < head {
                let events = store.event_page(None, Some(seq), head.checked_add(1), false, 128)?;
                let Some(last) = events.last() else { break };
                shared.devices().observe(&events);
                seq = last.seq;
            }
            seq = head;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("corrald event poll: {error:#}");
        }
    }
}

struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != ErrorKind::NotFound
        {
            eprintln!("remove daemon socket {}: {error}", self.0.display());
        }
    }
}

fn bind_socket(path: PathBuf) -> Result<Option<(File, SocketFile, UnixListener)>> {
    // The OS releases the advisory lock on exit, including a crash. This serializes
    // removing stale sockets; it does not own or gate any engine operation.
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("lock"))
        .context("open daemon socket lock")?;
    match lock.try_lock() {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(error) => return Err(error).context("acquire daemon socket lock"),
    }
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            ensure!(
                meta.file_type().is_socket(),
                "refusing to replace non-socket {}",
                path.display()
            );
            fs::remove_file(&path).context("remove stale daemon socket")?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => return Err(error).context("inspect daemon socket path"),
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind daemon socket {}", path.display()))?;
    let socket = SocketFile(path);
    fs::set_permissions(&socket.0, fs::Permissions::from_mode(0o600))
        .context("secure daemon socket")?;
    Ok(Some((lock, socket, listener)))
}

#[cfg(test)]
mod tests;

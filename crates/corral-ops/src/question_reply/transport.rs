use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use corral_core::Task;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Question, transcript};
use crate::Ctx;

const MAX_FRAME: u64 = 32 * 1024 * 1024;

pub(super) struct Connection {
    reader: BufReader<UnixStream>,
    sequence: u32,
}

impl Connection {
    pub fn open(socket: &Path) -> Result<Self> {
        let secret = transcript::read(&PathBuf::from(format!("{}.secret", socket.display())), 32)?;
        ensure!(secret.len() == 32, "native socket secret must be 32 bytes");
        use rustix::net::{
            AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with,
        };
        let fd = socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .context("opening native socket")?;
        // A full/stalled listen backlog must fail closed, not block the reply
        // operation indefinitely before its read/write timeouts can take effect.
        connect(
            &fd,
            &SocketAddrUnix::new(socket).context("encoding native socket address")?,
        )
        .context("connecting native question endpoint")?;
        let mut stream = UnixStream::from(fd);
        stream
            .set_nonblocking(false)
            .context("configuring native socket")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .context("bounding native read")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .context("bounding native write")?;
        stream
            .write_all(&secret)
            .context("authenticating native endpoint")?;
        Ok(Self {
            reader: BufReader::new(stream),
            sequence: 0,
        })
    }

    pub fn call(&mut self, mut command: Value) -> Result<Value> {
        self.sequence += 1;
        let id = format!("corral-{}", self.sequence);
        command["id"] = id.clone().into();
        let stream = self.reader.get_mut();
        serde_json::to_writer(&mut *stream, &command).context("encoding native command")?;
        stream.write_all(b"\n").context("sending native command")?;
        let mut bytes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "native acknowledgement deadline exceeded"
            );
            self.reader
                .get_mut()
                .set_read_timeout(Some(remaining))
                .context("bounding native acknowledgement")?;
            let buffer = self
                .reader
                .fill_buf()
                .context("reading native acknowledgement")?;
            ensure!(!buffer.is_empty(), "incomplete native acknowledgement");
            let count = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(buffer.len(), |end| end + 1);
            ensure!(
                u64::try_from(bytes.len() + count)? <= MAX_FRAME,
                "oversized native acknowledgement"
            );
            bytes.extend_from_slice(&buffer[..count]);
            self.reader.consume(count);
            if bytes.ends_with(b"\n") {
                break;
            }
        }
        let response: Value =
            serde_json::from_slice(&bytes).context("parsing native acknowledgement")?;
        ensure!(
            response["id"] == id
                && response["command"] == command["type"]
                && response["type"] == "response",
            "native acknowledgement identity mismatch"
        );
        ensure!(
            response["success"] == true,
            "native endpoint refused: {}",
            response["error"]
        );
        Ok(response["data"].clone())
    }

    pub fn check(&mut self, question: &Question) -> Result<()> {
        let protocol = self.call(json!({"type":"get_protocol_info"}))?;
        ensure!(
            protocol["mode"] == "tui"
                && protocol["instanceId"] == question.instance
                && protocol["commands"]
                    .as_array()
                    .is_some_and(|commands| commands.iter().any(|c| c == "extension_ui_response")),
            "native endpoint changed or does not support question answers; answer manually"
        );
        let listing = self.call(json!({"type":"list_sessions"}))?;
        let sessions = listing["sessions"]
            .as_array()
            .context("native session inventory missing")?;
        ensure!(
            sessions.len() == 1
                && sessions[0]["sessionId"] == question.session_id
                && sessions[0]["sessionPath"].as_str().map(Path::new)
                    == Some(question.session.as_path()),
            "native endpoint session changed; answer manually"
        );
        Ok(())
    }
}

#[derive(Deserialize)]
struct Endpoint {
    endpoint_kind: String,
    socket: PathBuf,
}

#[derive(Deserialize)]
struct Pointer {
    instance_id: String,
}

#[derive(Deserialize)]
struct Generation {
    pid: u32,
    socket: PathBuf,
    instance_id: String,
}

fn metadata<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&transcript::read(path, 64 * 1024)?)
        .with_context(|| format!("parsing {}", path.display()))
}

pub(super) fn check_pane(ctx: &Ctx, task: &Task, question: &Question) -> Result<()> {
    ensure!(
        crate::session_scope::owns_task(ctx, task)?,
        "selected herdr session does not own the native question pane"
    );
    let target = super::target(ctx, task)?;
    ensure!(
        ctx.herdr
            .pane_process_info(target.pane)?
            .foreground_processes
            .iter()
            .any(|process| process.pid == question.pid),
        "native question process left the reply pane; answer manually"
    );
    Ok(())
}

pub(super) fn discover(ctx: &Ctx, task: &Task) -> Result<Option<Question>> {
    let target = super::target(ctx, task)?;
    let agent = crate::omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
    let registry = agent.join("rpc-host-daemon");
    let entries = match std::fs::read_dir(&registry) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading native endpoint registry"),
    };
    let mut endpoints = Vec::new();
    for entry in entries {
        let entry = entry.context("reading native endpoint entry")?;
        if !entry
            .file_type()
            .context("inspecting native endpoint entry")?
            .is_dir()
        {
            continue;
        }
        let endpoint: Endpoint = metadata(&entry.path().join("endpoint.json"))?;
        if endpoint.endpoint_kind != "tui" {
            continue;
        }
        let pointer_path = entry.path().join("host.pid");
        if !pointer_path.exists() {
            continue;
        }
        let pointer: Pointer = metadata(&pointer_path)?;
        ensure!(
            !pointer.instance_id.contains(['/', '\\']) && !pointer.instance_id.is_empty(),
            "invalid native instance id"
        );
        let generation: Generation = metadata(
            &entry
                .path()
                .join("generations")
                .join(&pointer.instance_id)
                .join("host.pid"),
        )?;
        ensure!(
            generation.socket == endpoint.socket && generation.instance_id == pointer.instance_id,
            "native endpoint generation mismatch"
        );
        endpoints.push((endpoint, generation));
    }
    if endpoints.is_empty() {
        return Ok(None);
    }
    let processes = ctx.herdr.pane_process_info(target.pane)?;
    let matching: Vec<_> = endpoints
        .into_iter()
        .filter(|(_, generation)| {
            processes
                .foreground_processes
                .iter()
                .any(|p| p.pid == generation.pid)
        })
        .collect();
    ensure!(
        matching.len() <= 1,
        "multiple native endpoints in reply pane; answer manually"
    );
    let Some((endpoint, generation)) = matching.into_iter().next() else {
        return Ok(None);
    };
    ensure!(
        crate::session_scope::owns_task(ctx, task)?,
        "selected herdr session does not own the native question pane"
    );
    let mut connection = Connection::open(&endpoint.socket)?;
    let protocol = connection.call(json!({"type":"get_protocol_info"}))?;
    ensure!(
        protocol["instanceId"] == generation.instance_id && protocol["mode"] == "tui",
        "native endpoint identity mismatch"
    );
    let listing = connection.call(json!({"type":"list_sessions"}))?;
    let sessions = listing["sessions"]
        .as_array()
        .context("native session inventory missing")?;
    ensure!(
        sessions.len() == 1,
        "native endpoint does not identify one session"
    );
    let row = &sessions[0];
    ensure!(
        row["cwd"].as_str().map(Path::new) == Some(target.cwd.as_path()),
        "native session cwd differs from reply worktree"
    );
    let session = PathBuf::from(
        row["sessionPath"]
            .as_str()
            .context("native session has no transcript")?,
    );
    let snapshot = transcript::Snapshot::read(&session)?;
    ensure!(
        snapshot.cwd == target.cwd && row["sessionId"] == snapshot.session_id,
        "native transcript identity mismatch"
    );
    ensure!(
        snapshot.pending.len() <= 1,
        "multiple native questions pending; answer the exact request manually"
    );
    Ok(snapshot
        .pending
        .into_iter()
        .next()
        .map(|(request, identity)| Question {
            socket: endpoint.socket,
            pid: generation.pid,
            instance: generation.instance_id,
            session,
            session_id: snapshot.session_id,
            request,
            identity,
        }))
}

//! The corral side of bridge v1. Every request carries the generation's `ctl_cap`, read fresh
//! from its 0600 `ctl.cap`, and the host's peer uid must be this user's before anything is sent.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::agent::ContainerRef;
use corral_core::inbox::Recipient;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::launch::{CTL_CAP_FILE, SOCKET_FILE, generation_dir};

pub const BRIDGE_VERSION: u32 = 1;
pub const FRAME_MAX_BYTES: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(60);

/// A bridge refusal or failure with a stable code; reasons never hold a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeError {
    pub code: String,
    pub reason: String,
}

fn refused(code: &str, reason: impl Into<String>) -> anyhow::Error {
    BridgeError {
        code: code.into(),
        reason: reason.into(),
    }
    .into()
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bridge {}: {}", self.code, self.reason)
    }
}

impl std::error::Error for BridgeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyPolicy {
    Steer,
    FollowUp,
    Prompt,
}

impl BusyPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            BusyPolicy::Steer => "steer",
            BusyPolicy::FollowUp => "follow_up",
            BusyPolicy::Prompt => "prompt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitInput {
    pub text: String,
    pub busy_policy: BusyPolicy,
    pub idempotency_key: String,
    pub origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Started,
    Queued,
    Handled,
    Rejected,
    /// Admitted; upstream has not answered yet. The outcome follows as an `input_settled` event
    /// and as the answer to a replay of the same key.
    Pending,
}

/// The host's truthful answer to one input: never a completion claim.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SubmitReply {
    pub request_id: String,
    pub idempotency_key: String,
    pub disposition: Disposition,
    #[serde(default)]
    pub turn: Option<u64>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub replayed: bool,
}

/// Events the host still holds after a cursor, at most one frame of them: continue from
/// `next` while `more`. `gap` means some were evicted before this reader saw them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventsPage {
    pub generation: u32,
    pub events: Vec<Value>,
    pub last: u64,
    pub next: u64,
    pub more: bool,
    #[serde(default)]
    pub gap: Option<EventGap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct EventGap {
    pub after: u64,
    pub resumed: u64,
    pub missing: u64,
}

pub struct BridgeClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    ctl_cap: String,
    generation: u32,
    next: u64,
}

impl std::fmt::Debug for BridgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeClient")
            .field("generation", &self.generation)
            .field("ctl_cap", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Reads a generation's `ctl.cap`: a bounded 0600 regular file of this user.
pub fn read_ctl_cap(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refused(
                "control_credential_missing",
                format!("{} does not exist", path.display()),
            ));
        }
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    if !meta.is_file()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        return Err(refused(
            "control_credential_unsafe",
            format!("{} must be a 0600 file of this user", path.display()),
        ));
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .take(512)
        .read_to_string(&mut text)
        .with_context(|| format!("reading {}", path.display()))?;
    let valid = (32..=256).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid {
        return Err(refused(
            "control_credential_unsafe",
            format!("{} does not hold a control credential", path.display()),
        ));
    }
    Ok(text)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn check_peer(stream: &UnixStream) -> Result<()> {
    let peer = rustix::net::sockopt::socket_peercred(stream)
        .context("reading the host's peer credentials")?;
    peer_matches(peer.uid.as_raw(), rustix::process::getuid().as_raw())
}

/// The host must run as this user: same-user trust, as T-136 specified.
pub fn peer_matches(peer: u32, own: u32) -> Result<()> {
    if peer != own {
        return Err(refused(
            "peer_refused",
            format!("the socket's peer uid {peer} is not uid {own}"),
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn check_peer(_stream: &UnixStream) -> Result<()> {
    Err(refused(
        "peer_refused",
        "peer credentials are unavailable on this platform",
    ))
}

impl BridgeClient {
    /// Connects to generation `generation` of `recipient`'s host.
    pub fn connect(store: &Store, recipient: &Recipient, generation: u32) -> Result<BridgeClient> {
        Self::connect_dir(&generation_dir(store, recipient, generation)?, generation)
    }

    /// Connects through the `ctl.cap` and socket inside a generation directory.
    pub fn connect_dir(dir: &Path, generation: u32) -> Result<BridgeClient> {
        let ctl_cap = read_ctl_cap(&dir.join(CTL_CAP_FILE))?;
        let socket = dir.join(SOCKET_FILE);
        let stream = UnixStream::connect(&socket).map_err(|error| {
            refused("host_unreachable", format!("{}: {error}", socket.display()))
        })?;
        check_peer(&stream)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(BridgeClient {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            ctl_cap,
            generation,
            next: 0,
        })
    }

    /// One request and its reply; a host refusal is a `BridgeError` with the host's code.
    pub fn request(&mut self, command: &str, args: Value) -> Result<Value> {
        self.next += 1;
        let id = format!("cr-{}-{}", std::process::id(), self.next);
        let mut frame = serde_json::to_vec(&json!({
            "version": BRIDGE_VERSION, "id": id, "ctl_cap": self.ctl_cap,
            "generation": self.generation, "command": command, "args": args,
        }))?;
        frame.push(b'\n');
        if frame.len() > FRAME_MAX_BYTES {
            return Err(refused(
                "frame_too_large",
                format!("frames are at most {FRAME_MAX_BYTES} bytes"),
            ));
        }
        self.writer
            .write_all(&frame)
            .context("sending a bridge request")?;
        let mut line = Vec::new();
        (&mut self.reader)
            .take(FRAME_MAX_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .context("reading the bridge reply")?;
        if line.last() != Some(&b'\n') {
            return Err(refused(
                "host_closed",
                "the host closed the connection without a reply",
            ));
        }
        let reply: Value = serde_json::from_slice(&line)
            .map_err(|_| refused("bad_reply", "the host's reply is not JSON"))?;
        if reply["ok"] != true {
            let code = reply["error"]["code"].as_str().unwrap_or("bad_reply");
            let reason = reply["error"]["reason"].as_str().unwrap_or("");
            return Err(refused(code, reason.to_owned()));
        }
        if reply["id"] != json!(id) || reply["generation"] != json!(self.generation) {
            return Err(refused(
                "bad_reply",
                "the reply names another request or generation",
            ));
        }
        Ok(reply["result"].clone())
    }

    pub fn hello(&mut self) -> Result<Value> {
        self.request("hello", json!({}))
    }

    pub fn get_state(&mut self) -> Result<Value> {
        self.request("get_state", json!({}))
    }

    pub fn submit_input(&mut self, input: &SubmitInput) -> Result<SubmitReply> {
        let result = self.request(
            "submit_input",
            json!({
                "text": input.text, "busy_policy": input.busy_policy.as_str(),
                "idempotency_key": input.idempotency_key, "origin": input.origin,
            }),
        )?;
        serde_json::from_value(result).context("reading the submit_input disposition")
    }

    pub fn cancel(&mut self, turn: Option<u64>) -> Result<Value> {
        self.request("cancel", json!({"turn": turn}))
    }

    pub fn events_since(&mut self, seq: u64) -> Result<EventsPage> {
        let result = self.request("events_since", json!({"seq": seq}))?;
        serde_json::from_value(result).context("reading the events page")
    }

    pub fn shutdown(&mut self) -> Result<Value> {
        self.request("shutdown", json!({}))
    }
}

/// Events after `seq`; a gap is logged as `agent_events_gap`, the container's requirement to
/// reconcile from canonical records rather than guess what the missing events said.
pub fn events_since_recorded(
    client: &mut BridgeClient,
    store: &Store,
    container: &ContainerRef,
    seq: u64,
) -> Result<EventsPage> {
    let page = client.events_since(seq)?;
    if let Some(gap) = page.gap {
        store.log_event(
            container.task(),
            "agent_events_gap",
            json!({
                "container": container.key(), "generation": page.generation,
                "after": gap.after, "resumed": gap.resumed, "missing": gap.missing,
                "reconcile": true,
            }),
        )?;
    }
    Ok(page)
}

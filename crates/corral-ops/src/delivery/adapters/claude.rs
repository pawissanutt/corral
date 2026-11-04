//! Claude's cross-session inbox. Validated only on Linux, Claude Code 2.1.304,
//! peerProtocol 1 (docs/records/T-208-claude-capture). No terminal fallback on refusal.
mod policy;
mod process;
#[cfg(all(test, target_os = "linux"))]
mod tests;
mod transcript;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use corral_core::inbox::{Inbox, Recipient};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;
use crate::delivery::{
    BindOutcome, BlockReason, Capabilities, DeliveryAdapter, Envelope, Observation, ObserveCursor,
    Readiness, SendBudget, SendOutcome, SessionBinding,
};

pub const HARNESS: &str = "claude";
const VERSION: &str = "2.1.304";
const DEADLINE: Duration = Duration::from_secs(30);

/// `SessionBinding.session` is adapter-opaque: pin the kernel generation alongside the
/// native session id without changing the shared adapter contract or old serialized bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    session_id: String,
    pid: u32,
    proc_start: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    pid: u32,
    session_id: String,
    cwd: PathBuf,
    started_at: i64,
    proc_start: String,
    version: String,
    peer_protocol: u32,
    kind: String,
    messaging_socket_path: PathBuf,
}

impl Session {
    fn identity(&self) -> Identity {
        Identity {
            session_id: self.session_id.clone(),
            pid: self.pid,
            proc_start: self.proc_start.clone(),
        }
    }

    fn verified(&self) -> bool {
        cfg!(target_os = "linux") && self.version == VERSION && self.peer_protocol == 1
    }
}

pub struct ClaudeAdapter {
    directory: PathBuf,
}

impl ClaudeAdapter {
    pub fn from_env() -> Self {
        Self::with_directory(
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                        .join(".claude")
                }),
        )
    }

    pub fn with_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn binding(&self, session: &Session) -> Result<SessionBinding> {
        ensure!(safe_id(&session.session_id), "unsafe Claude session id");
        let slug: String = session
            .cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        Ok(SessionBinding {
            session: serde_json::to_string(&session.identity())
                .context("encoding Claude binding")?,
            endpoint: Some(session.messaging_socket_path.clone()),
            transcript: Some(
                self.directory
                    .join("projects")
                    .join(slug)
                    .join(format!("{}.jsonl", session.session_id)),
            ),
        })
    }

    fn sessions(&self, ctx: &Ctx, recipient: &Recipient) -> Result<Vec<Session>> {
        let target = super::omo::target(ctx, recipient)?;
        let Some(task) = recipient.task() else {
            return Ok(Vec::new());
        };
        if !crate::session_scope::owns_head(ctx, &ctx.store.head(task)?)? {
            return Ok(Vec::new());
        }
        let Some(pane) = target.pane.as_deref() else {
            return Ok(Vec::new());
        };
        let foreground = ctx.herdr.pane_process_info(pane)?.foreground_processes;
        let directory = self.directory.join("sessions");
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).context("reading Claude session registry"),
        };
        let mut sessions = Vec::new();
        for entry in entries {
            let entry = entry.context("reading Claude session entry")?;
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json")
                || !entry
                    .file_type()
                    .context("inspecting Claude registry entry")?
                    .is_file()
            {
                continue;
            }
            let session: Session = match process::read_json(&path) {
                Ok(session) => session,
                // A torn/obsolete registry entry is not binding evidence.
                Err(_) => continue,
            };
            if path.file_stem().and_then(|s| s.to_str()) != Some(&session.pid.to_string())
                || session.cwd != target.cwd
                || session.kind != "interactive"
                || !safe_id(&session.session_id)
            {
                continue;
            }
            let at = jiff::Timestamp::from_millisecond(session.started_at)
                .context("Claude startedAt out of range")?;
            let earliest = target
                .started
                .checked_sub(jiff::SignedDuration::from_secs(5))
                .unwrap_or(target.started);
            if at < earliest || at > target.ended.unwrap_or(ctx.store.now()) {
                continue;
            }
            if process::in_pane(&session, &foreground)?
                && process::safe_socket(&session.messaging_socket_path).is_ok()
            {
                sessions.push(session);
            }
        }
        Ok(sessions)
    }

    fn revalidate(
        &self,
        ctx: &Ctx,
        recipient: &Recipient,
        binding: &SessionBinding,
    ) -> Result<Session> {
        let identity: Identity =
            serde_json::from_str(&binding.session).context("invalid Claude binding")?;
        let mut sessions = self.sessions(ctx, recipient)?;
        ensure!(
            sessions.len() == 1,
            "Claude session is unbound or ambiguous"
        );
        let session = sessions.pop().context("Claude session disappeared")?;
        ensure!(
            session.identity() == identity && self.binding(&session)? == *binding,
            "Claude session generation changed"
        );
        ensure!(session.verified(), "Claude protocol_unverified");
        ensure!(
            policy::readiness(&session)? == Readiness::Native,
            "Claude inbound_policy blocked"
        );
        Ok(session)
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

impl DeliveryAdapter for ClaudeAdapter {
    fn harness(&self) -> &'static str {
        HARNESS
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: false,
            admission: false,
            non_waking: false,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
            send_budget: Some(SendBudget {
                per_recipient_interval: Duration::from_secs(10),
                per_minute_total: 6,
            }),
            transcript_reader: true,
        }
    }

    fn readiness(&self, ctx: &Ctx, recipient: &Recipient) -> Result<Readiness> {
        let sessions = self.sessions(ctx, recipient)?;
        Ok(match sessions.as_slice() {
            [] => Readiness::Blocked {
                reason: BlockReason::SessionUnbound,
            },
            [session] if !session.verified() => Readiness::Blocked {
                reason: BlockReason::ProtocolUnverified,
            },
            [session] => match policy::readiness(session) {
                Ok(readiness) => readiness,
                Err(error) => {
                    crate::messages::report(
                        recipient.task(),
                        "inbox",
                        format!("Claude inbound policy blocked: {error:#}"),
                    );
                    Readiness::Blocked {
                        reason: BlockReason::PolicyRefused,
                    }
                }
            },
            _ => Readiness::Blocked {
                reason: BlockReason::SessionAmbiguous,
            },
        })
    }

    fn bind(&self, ctx: &Ctx, recipient: &Recipient) -> Result<BindOutcome> {
        let sessions = self.sessions(ctx, recipient)?;
        Ok(match sessions.as_slice() {
            [] => BindOutcome::Unbound {
                reason: "no pane/process-bound Claude session".into(),
            },
            [session] => BindOutcome::Bound(self.binding(session)?),
            _ => BindOutcome::Ambiguous {
                candidates: sessions.iter().map(|s| s.session_id.clone()).collect(),
            },
        })
    }

    fn send(&self, ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        let prepared = (|| {
            ensure!(
                &envelope.binding == binding && envelope.adapter == HARNESS,
                "Claude envelope binding mismatch"
            );
            let key = envelope
                .idempotency_key
                .rsplit_once("@env")
                .context("missing envelope recipient")?
                .0;
            let recipient = Recipient::parse_key(key)?;
            let payload = Inbox::new(&ctx.store, recipient.clone())?.payload(envelope)?;
            let session = self.revalidate(ctx, &recipient, binding)?;
            let frame = json!({"type":"user", "session_id":session.session_id, "from":"corral",
                "message":{"role":"user", "content":payload}});
            let mut bytes = serde_json::to_vec(&frame).context("encoding Claude inbox frame")?;
            bytes.push(b'\n');
            Ok::<_, anyhow::Error>((session, bytes))
        })();
        let (session, bytes) = match prepared {
            Ok(value) => value,
            Err(error) => return rejected(format!("Claude send preflight: {error:#}")),
        };
        process::write_frame(&session, &bytes, DEADLINE)
    }

    fn observe(
        &self,
        ctx: &Ctx,
        binding: &SessionBinding,
        cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        transcript::observe(ctx, binding, cursor)
    }
}

fn rejected(reason: String) -> SendOutcome {
    SendOutcome::Rejected {
        reason,
        retryable_after_fix: true,
    }
}

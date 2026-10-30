//! The child side of an owner action. The executing `corral agent call` runs the reserved
//! command as its direct child with `CORRAL_OWNER_ACTION=<A-n>:<request>`; that child claims
//! the reservation once, before the command runs, and then holds `OwnerAuthority::ViaAssistant`
//! for its lifetime: the owner guards accept it in place of the shell check, and its events are
//! attributed to the owner via `assistant:A-<n>` with the cited owner message. Any other
//! process that inherits the variable (one the command starts in turn) ignores it.
pub mod kinds;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, ensure};
use corral_core::Store;
use corral_core::agent::{EVIDENCE_MAX, Executor, ReceiptEvidence, ReceiptState, RequestReceipt};
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::agent::effects;

pub const OWNER_ACTION_ENV: &str = "CORRAL_OWNER_ACTION";
const STARTED: &str = "started";
const CHILD: &str = "owner_action_child";

/// The `owner_authority` every event of a claimed owner action carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViaAssistant {
    pub session: String,
    pub generation: u32,
    pub request: String,
    pub kind: String,
    pub input: String,
    pub sha256: String,
    pub owner_message: String,
    pub confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<String>,
}

/// Who an owner-level command acts for: the owner's own entry point, or an assistant that
/// executes a receipted owner instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerAuthority {
    Shell,
    ViaAssistant(&'static ViaAssistant),
}

static AUTHORITY: OnceLock<ViaAssistant> = OnceLock::new();

/// This process's authority: `ViaAssistant` once it claimed an owner action, else the shell.
pub fn authority() -> OwnerAuthority {
    AUTHORITY
        .get()
        .map_or(OwnerAuthority::Shell, OwnerAuthority::ViaAssistant)
}

/// The command child a `started` evidence names, once the child claimed its reservation.
pub(crate) fn started(receipt: &RequestReceipt) -> Option<Executor> {
    let evidence = receipt
        .evidence
        .iter()
        .find(|e| e.state == STARTED && e.source == CHILD)?;
    let (pid, start_time) = evidence.detail.as_deref()?.split_once(' ')?;
    Some(Executor {
        pid: pid.parse().ok()?,
        start_time: start_time.into(),
    })
}

/// Claims `CORRAL_OWNER_ACTION` for this process when it names a reservation whose executor is
/// this process's parent, and registers the authority. `None` when the variable is absent or
/// belongs to another process.
pub fn claim_from_env(start: &Path) -> Result<Option<ViaAssistant>> {
    let Some(value) = std::env::var_os(OWNER_ACTION_ENV) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| anyhow!("{OWNER_ACTION_ENV} is not valid UTF-8"))?;
    #[cfg(unix)]
    let parent = std::os::unix::process::parent_id();
    #[cfg(not(unix))]
    let parent = 0;
    let store = Store::open_unobserved(&Store::find_root(start)?)?;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(via) = claim(&store, &value, &argv, parent)? else {
        return Ok(None);
    };
    AUTHORITY
        .set(via.clone())
        .map_err(|_| anyhow!("this process already claimed an owner action"))?;
    Ok(Some(via))
}

/// The claim itself: in one container transaction, the reservation must still be reserved by
/// `parent`, have started no command and name exactly `argv`; the claim records this process
/// as the command child. A refusal changes nothing, so the command never runs without it.
pub fn claim(
    store: &Store,
    value: &str,
    argv: &[String],
    parent: u32,
) -> Result<Option<ViaAssistant>> {
    let (session, request) = value
        .split_once(':')
        .with_context(|| format!("{OWNER_ACTION_ENV} must be <A-n>:<request>"))?;
    corral_core::assistant::validate_id(session)?;
    let me = effects::executor()?;
    store.settle_assistant_container(session, |assistant, _| {
        let Some(index) = assistant
            .state
            .receipts
            .iter()
            .position(|r| r.request_id == request && r.op == "owner_action")
        else {
            return Ok(None);
        };
        let receipt = &assistant.state.receipts[index];
        let Some(executor) = receipt.executor.clone() else {
            return Ok(None);
        };
        if executor.pid != parent || effects::executor_gone(&executor)? {
            return Ok(None);
        }
        ensure!(
            receipt.state == ReceiptState::Reserved && started(receipt).is_none(),
            "owner_action_unclaimed: {request} already started its command"
        );
        let expected: Vec<String> = serde_json::from_value(receipt.result["argv"].clone())
            .context("owner_action_unclaimed: the reservation names no command")?;
        ensure!(
            expected == argv,
            "owner_action_unclaimed: this command is not the one {request} reserved"
        );
        let text = |key: &str| receipt.result[key].as_str().unwrap_or_default().to_owned();
        let input = text("input");
        let owner_message = assistant
            .owner_input(&input)
            .map(|entry| entry.text.clone())
            .with_context(|| format!("owner_action_unclaimed: {input} left the journal"))?;
        let via = ViaAssistant {
            session: session.into(),
            generation: receipt.identity.generation,
            request: request.into(),
            kind: text("kind"),
            sha256: text("sha256"),
            input,
            owner_message,
            confirmed: receipt.result["confirmed"].as_bool().unwrap_or(false),
            confirmation: receipt.result["confirmation"].as_str().map(str::to_owned),
        };
        let receipt = &mut assistant.state.receipts[index];
        if receipt.evidence.len() >= EVIDENCE_MAX {
            receipt.evidence.remove(0);
        }
        receipt.evidence.push(ReceiptEvidence {
            state: STARTED.into(),
            source: CHILD.into(),
            detail: Some(format!("{} {}", me.pid, me.start_time)),
            at: jiff::Timestamp::now(),
        });
        Ok(Some(via))
    })
}

#[cfg(test)]
thread_local! {
    /// Test-only: the program run in place of the corral binary.
    pub(crate) static EXECUTABLE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

fn executable() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = EXECUTABLE.with(|path| path.borrow().clone()) {
        return Ok(path);
    }
    crate::prompt::corral_bin()
}

/// Runs the reserved command at the workspace root with no input and waits for it.
pub(crate) fn run(ctx: &Ctx, session: &str, request: &str, argv: &[String]) -> Result<Output> {
    let program = executable()?;
    Command::new(&program)
        .args(argv)
        .current_dir(ctx.store.root())
        .env(OWNER_ACTION_ENV, format!("{session}:{request}"))
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", program.display()))
}

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
const FINISHED: &str = "finished";
/// Bytes of a failed command's error a completion record keeps.
const ERROR_MAX: usize = 1024;
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
/// The workspace root of the claimed reservation, where `finish` records completion.
static CLAIMED_ROOT: OnceLock<PathBuf> = OnceLock::new();

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
    let root = Store::find_root(start)?;
    let store = Store::open_unobserved(&root)?;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(via) = claim(&store, &value, &argv, parent)? else {
        return Ok(None);
    };
    AUTHORITY
        .set(via.clone())
        .map_err(|_| anyhow!("this process already claimed an owner action"))?;
    CLAIMED_ROOT
        .set(root)
        .map_err(|_| anyhow!("this process already claimed an owner action"))?;
    Ok(Some(via))
}

/// Records how this command child's command ended on its claimed reservation (`error` is
/// none on success), once the command has returned. A no-op in any process that claimed
/// nothing. With it, a reservation whose executor died settles from this record rather than
/// from guesses; without it (the child died too), it stays uncertain.
pub fn finish(error: Option<&str>) -> Result<()> {
    let (Some(via), Some(root)) = (AUTHORITY.get(), CLAIMED_ROOT.get()) else {
        return Ok(());
    };
    finish_in(&Store::open_unobserved(root)?, via, error)
}

pub(crate) fn finish_in(store: &Store, via: &ViaAssistant, error: Option<&str>) -> Result<()> {
    let me = effects::executor()?;
    let error = error.map(|text| {
        let mut end = text.len().min(ERROR_MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    });
    store.settle_assistant_container(&via.session, |assistant, _| {
        let Some(receipt) = assistant
            .state
            .receipts
            .iter_mut()
            .find(|r| r.request_id == via.request)
        else {
            return Ok(());
        };
        let mine = started(receipt).is_some_and(|child| child == me);
        if receipt.state != ReceiptState::Reserved || !mine || completion(receipt).is_some() {
            return Ok(());
        }
        if receipt.evidence.len() >= EVIDENCE_MAX {
            receipt.evidence.remove(1);
        }
        receipt.evidence.push(ReceiptEvidence {
            state: FINISHED.into(),
            source: CHILD.into(),
            detail: Some(serde_json::json!({"ok": error.is_none(), "error": error}).to_string()),
            at: jiff::Timestamp::now(),
        });
        Ok(())
    })
}

/// The command child's completion record: `Some(None)` when the command succeeded,
/// `Some(Some(error))` when it failed, `None` when it recorded no completion.
pub(crate) fn completion(receipt: &RequestReceipt) -> Option<Option<String>> {
    let evidence = receipt
        .evidence
        .iter()
        .find(|e| e.state == FINISHED && e.source == CHILD)?;
    let record: serde_json::Value = serde_json::from_str(evidence.detail.as_deref()?).ok()?;
    if record["ok"].as_bool()? {
        Some(None)
    } else {
        Some(Some(
            record["error"].as_str().unwrap_or("refused").to_owned(),
        ))
    }
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

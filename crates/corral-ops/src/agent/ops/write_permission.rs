//! `write_permission` (T-656 grant-store): the assistant host's own op, never a model tool. The
//! payload's `action` picks one of three requests:
//!
//! - `decide` journals the owner's dialog answer as a `permission` owner input with its
//!   structured record, creates the grant (a once grant already consumed) and queues the audit
//!   line, all in one receipted transaction. Its request ID is derived from the session, the
//!   generation and a fresh invocation ID, and a stored decide receipt is answered only to a
//!   live generation: it is authority to write, not a record to read.
//! - `check` answers whether an active grant covers a target, and `list` returns the active
//!   grants. Both are authenticated lookups that store no receipt and require the caller's
//!   generation to be current and its session open.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::write_permissions::{
    self as wp, GrantLifetime, Home, Invocation, NewPermission, PERMISSION_PATH_MAX,
    PermissionChoice, PermissionPath, TOOL_CALL_MAX, WRITE_GRANTS_MAX, WriteGrant, WriteOperation,
    WriteTool,
};
use corral_core::agent::{
    AGENT_RECORD_VERSION, ReceiptEffect, ReceiptState, RequestOrigin, RequestReceipt,
};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::{self, Admission, Admitted, Mode};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::{Envelope, tool_request_id};
use crate::agent::receipts::{self, Commit, StoredAnswer, failpoint};
use crate::assistant::permissions::{drain_permission_audit, resolve_dir};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Decide {
        invocation: Invocation,
        tool: WriteTool,
        operation: WriteOperation,
        target: PermissionPath,
        digest: String,
        preview_sha256: String,
        choice: PermissionChoice,
        #[serde(default)]
        scope: Option<PermissionPath>,
    },
    Check {
        tool: WriteTool,
        target: PermissionPath,
    },
    List {},
}

struct Decide {
    invocation: Invocation,
    tool: WriteTool,
    operation: WriteOperation,
    target: PermissionPath,
    digest: String,
    preview_sha256: String,
    choice: PermissionChoice,
    scope: Option<PermissionPath>,
}

#[cfg(test)]
thread_local! {
    /// The wall clock decide, check and list read on this test thread.
    pub(crate) static CLOCK: std::cell::Cell<Option<Timestamp>> =
        const { std::cell::Cell::new(None) };
    /// The home directory the scope limit protects on this test thread.
    pub(crate) static HOME: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

fn now() -> Timestamp {
    #[cfg(test)]
    if let Some(now) = CLOCK.get() {
        return now;
    }
    Timestamp::now()
}

fn home() -> Home {
    #[cfg(test)]
    if let Some(home) = HOME.with_borrow(Clone::clone) {
        return Home::new(home);
    }
    Home::from_env()
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let action: Action = super::payload(
        envelope,
        "action (decide, check or list) and that action's fields",
    )?;
    match action {
        Action::Decide {
            invocation,
            tool,
            operation,
            target,
            digest,
            preview_sha256,
            choice,
            scope,
        } => decide(
            ctx,
            envelope,
            Decide {
                invocation,
                tool,
                operation,
                target,
                digest,
                preview_sha256,
                choice,
                scope,
            },
        ),
        Action::Check { tool, target } => check(ctx, envelope, tool, &target),
        Action::List {} => list(ctx, envelope),
    }
}

/// A path pair's expanded lexical form and normalized canonical form, each within its bound.
/// A `.corral`, `.git` or `.worktrees` component in either form is a fixed refusal.
fn forms(pair: &PermissionPath, what: &str, home: &Home) -> Result<(PathBuf, PathBuf)> {
    if [&pair.shown, &pair.canonical]
        .iter()
        .any(|path| path.is_empty() || path.len() > PERMISSION_PATH_MAX)
    {
        return Err(bad(format!(
            "{what} paths must be 1-{PERMISSION_PATH_MAX} bytes"
        )));
    }
    let lexical =
        wp::expand(&pair.shown, home).map_err(|why| bad(format!("{what}.shown {why}")))?;
    let canonical = wp::normalize(Path::new(&pair.canonical))
        .map_err(|why| bad(format!("{what}.canonical {why}")))?;
    for form in [&lexical, &canonical] {
        if let Some(component) = wp::protected_component(form) {
            return Err(Rejection::new(
                RejectCode::FixedRule,
                format!(
                    "{what} {} has a {component} component, which no permission can cover",
                    form.display()
                ),
            )
            .into());
        }
    }
    Ok((lexical, canonical))
}

/// The scope limit on both forms, the shown scope resolving to the stated canonical one in
/// corral's own resolution, and the canonical target inside it.
fn validate_scope(scope: &PermissionPath, target: &Path, home: &Home) -> Result<()> {
    let (lexical, canonical) = forms(scope, "scope", home)?;
    for form in [&lexical, &canonical] {
        wp::scope_limit(form, home)
            .map_err(|why| bad(format!("scope {} {why}", form.display())))?;
    }
    if resolve_dir(&lexical).as_deref() != Some(canonical.as_path()) {
        return Err(bad(
            "scope.canonical is not the directory scope.shown resolves to",
        ));
    }
    if !target.starts_with(&canonical) {
        return Err(bad("the target lies outside the scope"));
    }
    Ok(())
}

fn validate(envelope: &Envelope, d: &Decide, home: &Home) -> Result<()> {
    let identity = &envelope.identity;
    if !hex(&d.invocation.id, 32) {
        return Err(bad("invocation.id must be 32 lowercase hex digits"));
    }
    if d.invocation.generation != identity.generation {
        return Err(bad("invocation.generation must be the calling generation"));
    }
    let derived = tool_request_id(
        &identity.session_id,
        &format!("wp:{}:{}", d.invocation.generation, d.invocation.id),
    );
    if envelope.request_id != derived {
        return Err(bad(
            "a decide request_id is rq- and 32 hex digits of SHA-256(session_id, NUL, \
             wp:<generation>:<invocation id>)",
        ));
    }
    let call = &d.invocation.tool_call;
    if call.is_empty() || call.len() > TOOL_CALL_MAX || call.chars().any(char::is_control) {
        return Err(bad(format!(
            "invocation.tool_call must be 1-{TOOL_CALL_MAX} bytes of printable text"
        )));
    }
    if !d.operation.fits(d.tool) {
        return Err(bad(
            "write is new_file or overwrite; edit is edit with at least one replacement",
        ));
    }
    if !hex(&d.digest, 64) || !hex(&d.preview_sha256, 64) {
        return Err(bad("digest and preview_sha256 are lowercase SHA-256 hex"));
    }
    let (_, target) = forms(&d.target, "target", home)?;
    match (d.choice.lifetime(), &d.scope) {
        (Some(GrantLifetime::Hour | GrantLifetime::Session), Some(scope)) => {
            validate_scope(scope, &target, home)
        }
        (Some(GrantLifetime::Hour | GrantLifetime::Session), None) => {
            Err(bad("an hour or session decision names its scope"))
        }
        (_, Some(_)) => Err(bad("a once or deny decision takes no scope")),
        (_, None) => Ok(()),
    }
}

/// Every caller but a New decide leaves an undrained entry queued for the next drain.
fn drain_or_warn(ctx: &Ctx, session: &str) {
    if let Err(error) = drain_permission_audit(&ctx.store, session) {
        eprintln!("warning: {session}'s write-permission audit lines stay queued: {error:#}");
    }
}

fn decide(ctx: &Ctx, envelope: &Envelope, d: Decide) -> Result<CallResponse> {
    let home = home();
    validate(envelope, &d, &home)?;
    let session = envelope.identity.assistant.clone().unwrap_or_default();
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            ctx.store
                .inspect_assistant_container(&session, |container, ended| {
                    admission::recheck_assistant(container, ended, envelope.identity.generation)?;
                    crate::assistant::workflow::boundary::require_ordinary(
                        &ctx.store, &session, container,
                    )
                })?;
            let answer = receipts::replay_assistant_live(ctx, envelope, *stored);
            drain_or_warn(ctx, &session);
            return answer;
        }
        Admission::New(admitted) => admitted,
    };
    failpoint("write_permission_admitted_before_commit");
    // A new decision first flushes older ones, so the outbox stays bounded by the log.
    drain_permission_audit(&ctx.store, &session).context(
        "write_permission: queued audit lines could not be logged, so no new decision is taken",
    )?;
    let authorized = d.choice != PermissionChoice::Deny;
    let new = NewPermission {
        generation: envelope.identity.generation,
        turn: envelope.identity.turn,
        request: envelope.request_id.clone(),
        tool: d.tool,
        operation: d.operation,
        target: d.target,
        digest: d.digest,
        invocation: d.invocation,
        choice: d.choice,
        scope: d.scope,
    };
    let now = now();
    let committed =
        receipts::commit_assistant_with(ctx, &admitted, StoredAnswer::LiveOnly, |assistant| {
            crate::assistant::workflow::boundary::require_ordinary(
                &ctx.store, &session, assistant,
            )?;
            let (entry, grant) = assistant
                .record_permission(&session, new, now)
                .map_err(|_| {
                    Rejection::new(
                        RejectCode::GrantsFull,
                        format!(
                            "{session} holds {WRITE_GRANTS_MAX} active write grants; revoke one \
                             with `corral assistant permissions revoke`"
                        ),
                    )
                })?;
            let mut result = json!({"input": entry.id, "authorized": authorized});
            if let Some(grant) = &grant {
                result["grant"] = json!(grant.id);
                result["lifetime"] = json!(grant.lifetime);
                if let Some(expires) = grant.expires_at {
                    result["expires_at"] = json!(expires);
                }
            }
            Ok(Commit {
                result,
                effect: Some(ReceiptEffect {
                    kind: "write_permission".into(),
                    reference: entry.id,
                }),
                post_pending: Vec::new(),
            })
        });
    if committed.is_ok() {
        failpoint("write_permission_committed_before_audit");
    }
    drain_or_warn(ctx, &session);
    committed
}

fn lookup(ctx: &Ctx, envelope: &Envelope) -> Result<Box<Admitted>> {
    match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::New(admitted) => Ok(admitted),
        Admission::Replay(_) => Err(bad(
            "check and list store no receipt; send them with a fresh request_id",
        )),
    }
}

/// The session's grant records, read under the store lock after the caller's generation is
/// shown to be current, unfenced and in an open session.
fn live_grants(ctx: &Ctx, envelope: &Envelope, authorize: bool) -> Result<Vec<WriteGrant>> {
    let id = envelope
        .identity
        .assistant
        .as_deref()
        .context("a write_permission request names its assistant session")?;
    ctx.store
        .inspect_assistant_container(id, |assistant, ended| {
            admission::recheck_assistant(assistant, ended, envelope.identity.generation)?;
            if authorize {
                crate::assistant::workflow::boundary::require_ordinary(&ctx.store, id, assistant)?;
            }
            Ok(assistant.write_grants.clone())
        })
}

/// A lookup answer shaped as a committed receipt that is never stored.
fn unstored(origin: &RequestOrigin, result: Value) -> CallResponse {
    let at = Timestamp::now();
    CallResponse::Receipt {
        receipt: RequestReceipt {
            version: AGENT_RECORD_VERSION,
            request_id: origin.request_id.clone(),
            op: origin.op.clone(),
            payload_sha256: origin.payload_sha256.clone(),
            identity: origin.identity.clone(),
            state: ReceiptState::Committed,
            result,
            executor: None,
            effect: None,
            evidence: Vec::new(),
            post_pending: Vec::new(),
            at,
            committed_at: Some(at),
        },
        replayed: false,
    }
}

fn check(
    ctx: &Ctx,
    envelope: &Envelope,
    tool: WriteTool,
    target: &PermissionPath,
) -> Result<CallResponse> {
    let home = home();
    let (_, canonical) = forms(target, "target", &home)?;
    let admitted = lookup(ctx, envelope)?;
    let grants = live_grants(ctx, envelope, true)?;
    let result = match wp::check(&grants, &canonical, now(), &home, resolve_dir) {
        Some(grant) => {
            let mut found = json!({"tool": tool, "grant": grant.id, "scope": grant.scope});
            if let Some(expires) = grant.expires_at {
                found["expires_at"] = json!(expires);
            }
            found
        }
        None => json!({"tool": tool, "reason": "no_grant"}),
    };
    Ok(unstored(&admitted.origin, result))
}

fn list(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let admitted = lookup(ctx, envelope)?;
    drain_or_warn(
        ctx,
        envelope.identity.assistant.as_deref().unwrap_or_default(),
    );
    let grants = live_grants(ctx, envelope, false)?;
    let now = now();
    let active: Vec<&WriteGrant> = grants
        .iter()
        .filter(|grant| grant.lifetime != GrantLifetime::Once && grant.active(now))
        .collect();
    Ok(unstored(&admitted.origin, json!({"grants": active})))
}

#[cfg(test)]
#[path = "write_permission_tests.rs"]
mod tests;

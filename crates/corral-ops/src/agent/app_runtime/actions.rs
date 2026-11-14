//! Chat filing and withdrawal shared by app actions and the real assistant idea_file tool.
//! Filing uses the canonical task service, preserving verbatim owner words and assistant
//! origin. A task and its input-keyed duplicate guard publish in the same writer unit, so
//! a crash before the thread card updates cannot create a second task on recovery.
//! Concurrent filing refuses before reading duplicate guards or writing; the exclusive claim
//! spans task publication and all guard writes, including replay reconciliation.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentOrigin, ReceiptEffect, paths::sha256_hex};
use corral_core::{Origin, Status, records::RecordArea};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{AuthenticatedDevice, input};
use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::CallResponse;
use crate::agent::envelope::Envelope;
use crate::agent::receipts::{self, Commit};

const FILINGS: RecordArea<Filing> = RecordArea::new("app_idea_filings");
const HOST_FILINGS: RecordArea<HostFiling> = RecordArea::new("app_idea_host_filings");

#[path = "filing_log.rs"]
mod filing_log;
pub use filing_log::FilingLogPage;

/// Required, provider-free startup bootstrap of the global derived filing references.
pub fn ensure_filing_log(ctx: &Ctx) -> Result<()> {
    filing_log::ensure_filing_log(ctx)
}

/// Bounded point reads only; bootstrap is never hidden inside a normal page read.
pub fn filing_log(ctx: &Ctx, before: Option<u64>) -> Result<FilingLogPage> {
    filing_log::filing_log(ctx, before)
}

#[derive(Serialize, Deserialize)]
struct HostFiling {
    task: String,
    payload_sha256: String,
}

fn host_key(session: &str, request: &str) -> String {
    sha256_hex(format!("{session}\0{request}").as_bytes())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filing {
    pub task: String,
    pub session: String,
    pub input: String,
    pub device: Option<String>,
    pub name: Option<String>,
    pub words_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Idea {
    pub input: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub criteria: Vec<String>,
}

fn guard_key(session: &str, input: &str) -> String {
    sha256_hex(format!("{session}\0{input}").as_bytes())
}

pub fn filing(ctx: &Ctx, session: &str, input: &str) -> Result<Option<Filing>> {
    FILINGS.load(&ctx.store, &guard_key(session, input))
}

fn validate(idea: &Idea) -> Result<()> {
    ensure!(
        !idea.title.trim().is_empty() && idea.title.len() <= 200,
        "idea title must be 1..200 bytes"
    );
    ensure!(
        idea.body.len() <= 16 * 1024,
        "idea body exceeds 16384 bytes"
    );
    ensure!(
        idea.labels.len() <= 16 && idea.labels.iter().all(|v| !v.is_empty() && v.len() <= 64),
        "invalid idea labels"
    );
    ensure!(
        idea.criteria.len() <= 16
            && idea
                .criteria
                .iter()
                .all(|v| !v.trim().is_empty() && v.len() <= 1024),
        "invalid acceptance criteria"
    );
    Ok(())
}

/// Canonical real idea filing, not a command adapter. Model callers must use host_call, which
/// rechecks their generation inside the task writer unit. Direct app filing may preserve a
/// late/non-actionable message as written without granting it model execution authority.
fn file(ctx: &Ctx, session: &str, idea: &Idea, host: Option<&Envelope>) -> Result<String> {
    validate(idea)?;
    let _filing = ctx
        .store
        .try_lock("app-idea-filing")?
        .context("idea filing is already in progress")?;
    filing_log::ensure_locked(ctx)?;
    let key = guard_key(session, &idea.input);
    if let Some(host) = host
        && let Some(saved) = HOST_FILINGS.load(&ctx.store, &host_key(session, &host.request_id))?
    {
        ensure!(
            saved.payload_sha256 == host.payload_sha256,
            "payload_mismatch: idea_file request reused"
        );
        return Ok(saved.task);
    }
    if let Some(saved) = FILINGS.load(&ctx.store, &key)? {
        if let Some(host) = host {
            HOST_FILINGS.save(
                &ctx.store,
                &host_key(session, &host.request_id),
                &HostFiling {
                    task: saved.task.clone(),
                    payload_sha256: host.payload_sha256.clone(),
                },
            )?;
        }
        return Ok(saved.task);
    }
    let assistant = ctx.store.get_assistant_container(session)?;
    let entry = match assistant
        .owner_inputs
        .into_iter()
        .find(|entry| entry.id == idea.input)
    {
        Some(entry) => entry,
        None => input::owner_input(&ctx.store, session, &idea.input)?,
    };
    ensure!(
        matches!(
            entry.source,
            corral_core::assistant::OwnerInputSource::Pane
                | corral_core::assistant::OwnerInputSource::Dialog
        ) && !entry.truncated,
        "idea requires complete owner words, not a permission, confirmation or notice"
    );
    let source = input::provenance(&ctx.store, &entry.id)?;
    // Filing is not owner_action authority: a web input with assistant_act=false can still
    // become an idea, but it can never authorize queue/land/config/permission effects.
    if host.is_some()
        && assistant
            .state
            .current_generation()
            .is_some_and(|g| g.daemon_hosted)
    {
        ensure!(
            source.is_some(),
            "remote idea filing requires server-journaled owner words"
        );
    }
    let session_record = ctx.store.get_assistant(session)?;
    let mut body = idea.body.clone();
    if !idea.criteria.is_empty() {
        body.push_str("\n\n## Acceptance criteria\n");
        for criterion in &idea.criteria {
            body.push_str("- ");
            body.push_str(criterion);
            body.push('\n');
        }
    }
    body.push_str("\n\n## Owner words (verbatim)\n");
    body.push_str(&entry.text);
    if let Some(origin) = &source
        && let Some(id) = ctx.store.app_send_entry(&origin.device, &origin.cmd_id)?
    {
        let message = ctx.store.app_message(&id)?;
        if !message.attachments.is_empty() {
            body.push_str("\n\n## Attachments (untrusted, not owner words)\n");
            for attachment in &message.attachments {
                body.push_str(&format!(
                    "- {} {} {} {} bytes {}\n",
                    attachment.id,
                    attachment.kind,
                    serde_json::to_string(&attachment.name)?,
                    attachment.bytes,
                    serde_json::to_string(
                        &attachment
                            .sha256
                            .as_deref()
                            .or(attachment.url.as_deref())
                            .unwrap_or_default()
                    )?
                ));
            }
        }
    }
    body.push_str("\n\n## Filing provenance\n");
    body.push_str(&format!(
        "Assistant: {session}\nOwner input: {}\nSHA-256: {}\n",
        entry.id, entry.sha256
    ));
    if let Some(origin) = &source {
        body.push_str(&format!(
            "Device: {} ({})\nTyped at: {}\n",
            origin.device, origin.name, origin.typed_at
        ));
    }
    let task = crate::new_task_initialized(
        ctx,
        &idea.title,
        session_record.project.as_deref(),
        ctx.store.root(),
        Status::Idea,
        &body,
        |task| {
            if let Some(host) = host {
                ctx.store
                    .stage_assistant_container(session, |assistant, ended| {
                        admission::recheck_assistant(assistant, ended, host.identity.generation)?;
                        crate::assistant::workflow::boundary::require_ordinary(
                            &ctx.store, session, assistant,
                        )?;
                        if let Some(current) = assistant.owner_input(&idea.input) {
                            ensure!(
                                current.sha256 == entry.sha256,
                                "owner input changed before filing"
                            );
                        } else {
                            ensure!(
                                input::owner_input(&ctx.store, session, &idea.input)?.sha256
                                    == entry.sha256,
                                "owner input changed before filing"
                            );
                        }
                        Ok(())
                    })?;
                task.agent_origin = Some(AgentOrigin {
                    request_id: host.request_id.clone(),
                    session_id: host.identity.session_id.clone(),
                    generation: host.identity.generation,
                    at: Timestamp::now(),
                });
                HOST_FILINGS.save(
                    &ctx.store,
                    &host_key(session, &host.request_id),
                    &HostFiling {
                        task: task.id.clone(),
                        payload_sha256: host.payload_sha256.clone(),
                    },
                )?;
            }
            task.origin = Some(Origin::Assistant);
            task.labels = idea.labels.clone();
            FILINGS.save(
                &ctx.store,
                &key,
                &Filing {
                    task: task.id.clone(),
                    session: session.into(),
                    input: entry.id.clone(),
                    device: source.as_ref().map(|p| p.device.clone()),
                    name: source.as_ref().map(|p| p.name.clone()),
                    words_sha256: entry.sha256.clone(),
                },
            )?;
            filing_log::append(ctx, session, &entry.id)
        },
    )?;
    Ok(task.id)
}

/// Explicit owner fallback while the provider is unavailable. The message claim and worker
/// delivery compete on the same canonical app-message lock. A saved replay returns its task.
pub fn file_as_idea(ctx: &Ctx, device: &AuthenticatedDevice, entry: &str) -> Result<String> {
    ensure!(device.act, "authority_act");
    input::recheck_direct(&ctx.store, &device.id)?;
    // Build outside the message's writer unit so migration keeps its bounded batches.
    ensure_filing_log(ctx)?;
    let message = ctx.store.update_app_message(entry, |message| {
        input::recheck_direct(&ctx.store, &device.id)?;
        if message.saved_task.is_some() {
            return Ok(());
        }
        ensure!(
            matches!(
                message.state.as_str(),
                "sent" | "waiting" | "late" | "failed"
            ),
            "message has already been delivered or withdrawn"
        );
        let input_id = message
            .input
            .as_ref()
            .context("message owner input is not journaled")?;
        let first = message
            .text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("Owner idea");
        let title = super::excerpt(first, 120);
        let idea = Idea {
            input: input_id.clone(),
            title,
            body: String::new(),
            labels: Vec::new(),
            criteria: Vec::new(),
        };
        let task = file(ctx, &message.session, &idea, None)?;
        message.saved_task = Some(task);
        message.state = "saved".into();
        Ok(())
    })?;
    message.saved_task.context("idea filing returned no task")
}

/// Withdraw only an assistant-target message that has not entered delivery. PM messages use
/// the canonical pm.inbox-withdraw service owned by the PM-target runtime instead.
pub fn withdraw(ctx: &Ctx, device: &AuthenticatedDevice, entry: &str) -> Result<()> {
    ensure!(device.act, "authority_act");
    input::recheck_direct(&ctx.store, &device.id)?;
    ctx.store.update_app_message(entry, |message| {
        input::recheck_direct(&ctx.store, &device.id)?;
        ensure!(
            message.target == "assistant",
            "PM messages use pm.inbox-withdraw"
        );
        if message.state == "withdrawn" {
            return Ok(());
        }
        ensure!(
            matches!(
                message.state.as_str(),
                "sent" | "waiting" | "late" | "failed"
            ),
            "only waiting messages may be withdrawn"
        );
        message.state = "withdrawn".into();
        Ok(())
    })?;
    Ok(())
}

/// Real assistant tool: canonical generation admission, input-keyed task creation and receipt.
/// If a process dies after task creation, the next identical call finds the same filing guard.
pub fn host_call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let idea: Idea =
        serde_json::from_value(envelope.payload.clone()).context("parsing idea_file payload")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let session = envelope
        .identity
        .assistant
        .as_deref()
        .context("idea_file requires assistant session")?;
    let task = file(ctx, session, &idea, Some(envelope))?;
    receipts::commit_assistant(ctx, &admitted, |_| {
        Ok(Commit {
            result: json!({"kind": "idea_file", "task": task, "input": idea.input}),
            effect: Some(ReceiptEffect {
                kind: "idea_file".into(),
                reference: task,
            }),
            post_pending: Vec::new(),
        })
    })
}

#[cfg(test)]
#[path = "actions_global_tests.rs"]
mod global_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_owner_words_file_once_after_contention_and_citation_journal_eviction() {
        let fx = crate::agent::test_support::fx();
        let session = fx
            .ctx
            .store
            .create_assistant("pm-pi", None, None, None)
            .unwrap();
        let device = AuthenticatedDevice {
            id: "dev-web".into(),
            name: "Browser".into(),
            assistant: true,
            assistant_act: false,
            act: true,
            reply: true,
        };
        let words = "  Keep my original words — 日本語\nand this trailing whitespace.  \n";
        let entry = input::journal(
            &fx.ctx.store,
            &session.id,
            &device,
            "typed-once",
            words,
            &Timestamp::now().to_string(),
            false,
        )
        .unwrap();
        fx.ctx
            .store
            .update_assistant_container(&session.id, |assistant| {
                assistant.owner_inputs.clear();
                Ok(())
            })
            .unwrap();
        let mut idea = Idea {
            input: entry.id,
            title: "Keep original words".into(),
            body: "Structured explanation".into(),
            labels: vec!["chat".into()],
            criteria: Vec::new(),
        };
        let claim = fx.ctx.store.try_lock("app-idea-filing").unwrap().unwrap();
        let error = file(&fx.ctx, &session.id, &idea, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("idea filing is already in progress")
        );
        assert!(fx.ctx.store.list().unwrap().is_empty());
        assert!(filing(&fx.ctx, &session.id, &idea.input).unwrap().is_none());
        drop(claim);
        let task = file(&fx.ctx, &session.id, &idea, None).unwrap();
        idea.title = "A repeated model draft must not create a duplicate".into();
        assert_eq!(file(&fx.ctx, &session.id, &idea, None).unwrap(), task);
        let saved = fx.ctx.store.get(&task).unwrap();
        assert_eq!(saved.origin, Some(Origin::Assistant));
        assert!(saved.body.contains(words));
        assert_eq!(fx.ctx.store.list().unwrap().len(), 1);
    }
}

//! Bounded, provider-free read projections shared by daemon topics and detail readers.
use anyhow::{Context, Result};
use corral_core::assistant::AssistantSession;
use corral_core::assistant::threads::{Message, Thread};
use corral_protocol::assistant::{
    ContextRef, EntryCard, EntryKind, EntryPart, MessageState, ThreadEntry,
};
use corral_protocol::rows::{
    AssistantKind, AssistantLocation, AssistantRow, AssistantState, WaitReason,
};
#[path = "cards.rs"]
mod cards;
use crate::Ctx;
use corral_protocol::subscription::LiveText;

pub(crate) fn message_state(message: &Message) -> MessageState {
    if (message.late || message.tainted) && !matches!(message.state.as_str(), "withdrawn" | "saved")
    {
        return MessageState::Late;
    }
    match message.state.as_str() {
        "sent" => MessageState::Sent,
        "delivered" | "working" | "done" => MessageState::Delivered,
        "late" => MessageState::Late,
        "queued_pm" => MessageState::QueuedForPm,
        "pm_delivered" => MessageState::PmDelivered,
        "pm_replied" => MessageState::PmReplied,
        "saved" => MessageState::Saved {
            task: message.saved_task.clone().unwrap_or_default(),
        },
        "withdrawn" => MessageState::Withdrawn,
        _ => MessageState::Waiting {
            reason: WaitReason {
                code: if message.state == "uncertain" {
                    "uncertain".into()
                } else if message.state == "failed" {
                    message
                        .failure
                        .as_deref()
                        .and_then(|f| f.split_once(':'))
                        .map(|(code, _)| code)
                        .unwrap_or("unavailable")
                        .into()
                } else {
                    "unavailable".into()
                },
                text: message
                    .failure
                    .clone()
                    .unwrap_or_else(|| "Waiting for assistant".into()),
                retry_at: None,
            },
        },
    }
}

pub(crate) fn row(ctx: &Ctx, id: &str) -> Result<AssistantRow> {
    Ok(row_with_usage(ctx, id)?.0)
}

/// One session's row and its total tokens.
pub(crate) fn row_with_usage(ctx: &Ctx, id: &str) -> Result<(AssistantRow, Option<u64>)> {
    let session = ctx.store.get_assistant(id)?;
    let head = ctx.store.app_thread_head(id).ok();
    let mut row = base_row(ctx, &session, head)?;
    let tokens = add_usage(ctx, &session, &mut row)?;
    Ok((row, tokens))
}

/// Fills the row's cost from the session's transcripts and returns its total tokens. This is
/// the expensive part of a row, so feeds call it only for the rows they keep.
fn add_usage(ctx: &Ctx, session: &AssistantSession, row: &mut AssistantRow) -> Result<Option<u64>> {
    let usage = crate::usage::assistant_usage(&ctx.store, session)?;
    row.cost_usd = usage.as_ref().and_then(|usage| usage.cost_usd);
    row.cost_basis = usage
        .as_ref()
        .and_then(|usage| usage.cost_basis)
        .map(|basis| match basis {
            corral_core::CostBasis::Reported => corral_protocol::board::CostBasis::Reported,
            corral_core::CostBasis::Estimated => corral_protocol::board::CostBasis::Estimated,
            corral_core::CostBasis::Mixed => corral_protocol::board::CostBasis::Mixed,
        });
    Ok(usage.as_ref().map(|usage| usage.total_tokens()))
}

fn state(session: &AssistantSession, head: Option<&Thread>) -> AssistantState {
    if session.ended.is_some() {
        return AssistantState::Ended;
    }
    match head.map(|h| h.state.as_str()) {
        Some("starting") => AssistantState::Starting,
        Some("working") => AssistantState::Working,
        Some("ready") => AssistantState::Ready,
        Some("waiting") => AssistantState::Waiting,
        Some("paused") => AssistantState::Paused,
        Some("failed") => AssistantState::Failed,
        Some("stopping" | "takeover") => AssistantState::Stopping,
        _ => AssistantState::Cold,
    }
}

fn title_text(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let title: String = line
        .chars()
        .filter(|ch| !ch.is_control())
        .take(120)
        .collect();
    (!title.trim().is_empty()).then_some(title)
}

fn title(ctx: &Ctx, session: &AssistantSession, head: Option<&Thread>) -> Result<String> {
    if let Some(title) = title_text(&session.summary) {
        return Ok(title);
    }
    if head.is_some_and(|head| head.next > 1) {
        let first = ctx.store.app_message(&format!("{}/M1", session.id))?;
        if let Some(title) = title_text(&first.text) {
            return Ok(title);
        }
    }
    Ok(format!("{} · {}", session.id, session.focus))
}

/// The session's row without usage.
fn base_row(ctx: &Ctx, session: &AssistantSession, head: Option<Thread>) -> Result<AssistantRow> {
    let id = session.id.as_str();
    let container = ctx.store.get_assistant_container(id)?;
    let quick = head.as_ref().is_some_and(|h| h.quick)
        && ctx.store.app_thread_scope("quick")?.as_deref() == Some(id);
    let hosted = container
        .state
        .generation(container.state.binding.generation)
        .is_some_and(|g| g.daemon_hosted);
    let state = state(session, head.as_ref());
    // Match `assistant_stats`/CLI: a saved ready state or pane is not a live host.
    let live = session.ended.is_none() && super::super::pi::host_alive(&container.state)?;
    let state = if !live
        && matches!(
            state,
            AssistantState::Ready
                | AssistantState::Working
                | AssistantState::Waiting
                | AssistantState::Stopping
        ) {
        AssistantState::Cold
    } else {
        state
    };
    let turn = container.state.turns.last().map(|t| t.seq);
    let row = AssistantRow {
        id: id.into(),
        kind: if quick {
            AssistantKind::Quick
        } else {
            AssistantKind::Session
        },
        title: if quick {
            "Quick thread".into()
        } else {
            title(ctx, session, head.as_ref())?
        },
        state,
        live_in: if !live {
            AssistantLocation::None
        } else if hosted {
            AssistantLocation::Daemon
        } else if container.pane.is_some() {
            AssistantLocation::Pane
        } else {
            AssistantLocation::None
        },
        profile: Some(session.profile.clone()),
        connection: container
            .state
            .current_generation()
            .and_then(|generation| generation.connection.as_ref())
            .map(|binding| binding.connection.id().to_owned()),
        focus: (session.focus != "workspace").then(|| session.focus.clone()),
        pending: head.as_ref().map_or(0, |h| h.pending.len() as u32),
        turn,
        cost_usd: None,
        cost_basis: None,
        wait: head
            .as_ref()
            .and_then(|h| h.failure.as_ref())
            .map(|text| WaitReason {
                code: "unavailable".into(),
                text: text.clone(),
                retry_at: None,
            }),
        rev: head.map_or(container.state.binding.generation as u64, |h| h.rev),
    };
    Ok(row)
}

pub fn sessions(ctx: &Ctx) -> Result<Vec<AssistantRow>> {
    Ok(sessions_with_usage(ctx, |_, _| true)?
        .into_iter()
        .map(|(row, _)| row)
        .collect())
}

/// One canonical accounting pass supplies both the assistant and agent feed adapters. `keep`
/// sees each session's state and end time before its container and transcripts are read, so
/// a session a feed leaves out costs one thread-head read at most.
pub(crate) fn sessions_with_usage(
    ctx: &Ctx,
    keep: impl Fn(AssistantState, Option<jiff::Timestamp>) -> bool,
) -> Result<Vec<(AssistantRow, Option<u64>)>> {
    let mut rows = Vec::new();
    for session in ctx.store.list_assistants()? {
        if session.ended.is_some() && !keep(AssistantState::Ended, session.ended) {
            continue;
        }
        let head = ctx.store.app_thread_head(&session.id).ok();
        if !keep(state(&session, head.as_ref()), session.ended) {
            continue;
        }
        let mut row = base_row(ctx, &session, head)?;
        let tokens = add_usage(ctx, &session, &mut row)?;
        rows.push((row, tokens));
    }
    rows.sort_by(|a, b| {
        (a.0.kind != AssistantKind::Quick, &a.0.id)
            .cmp(&(b.0.kind != AssistantKind::Quick, &b.0.id))
    });
    Ok(rows)
}

fn text_parts(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![text];
    }
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(7 * 1024);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(&rest[..end]);
        rest = &rest[end..];
    }
    parts
}

fn entries(ctx: &Ctx, snapshot: &cards::Snapshot, message: &Message) -> Result<Vec<ThreadEntry>> {
    let mut rows = Vec::new();
    let owner_parts = text_parts(&message.text);
    let owner_total = owner_parts.len() as u32;
    for (part, text) in owner_parts.into_iter().enumerate() {
        rows.push(ThreadEntry {
            id: if part == 0 {
                message.id.clone()
            } else {
                format!("{}/text/{part}", message.id)
            },
            rev: message.rev,
            at: message.at.clone(),
            typed_at: Some(message.typed_at.clone()),
            kind: if message.target == "pm" {
                EntryKind::PmMessage
            } else {
                EntryKind::Message
            },
            device: Some(message.device.clone()),
            via: Some(format!("app:{}", message.device)),
            text: Some(text.into()),
            state: Some(message_state(message)),
            model: message.model.clone(),
            cost_usd: None,
            cost_basis: None,
            completion: None,
            part: Some(EntryPart {
                id: message.id.clone(),
                index: part as u32,
                total: owner_total,
                bytes: message.text.len() as u64,
                complete: true,
            }),
            tools: Vec::new(),
            links: message
                .context
                .iter()
                .map(|c| ContextRef {
                    kind: c.kind.clone(),
                    id: c.id.clone(),
                    rev: c.rev,
                })
                .collect(),
            attachments: if part == 0 {
                message
                    .attachments
                    .iter()
                    .map(|a| corral_protocol::attachment::AttachmentFact {
                        id: a.id.clone(),
                        kind: match a.kind.as_str() {
                            "image" => corral_protocol::attachment::AttachmentKind::Image,
                            "file" => corral_protocol::attachment::AttachmentKind::File,
                            "link" => corral_protocol::attachment::AttachmentKind::Link,
                            _ => corral_protocol::attachment::AttachmentKind::Other,
                        },
                        name: a.name.clone(),
                        media_type: a.media_type.clone(),
                        bytes: (a.kind != "link").then_some(a.bytes),
                        sha256: a.sha256.clone(),
                        url: a.url.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            card: message
                .pm_item
                .as_ref()
                .map(|id| {
                    ctx.store.pm_inbox_item(id).map(|item| EntryCard::PmItem {
                        id: id.clone(),
                        revision: item.revision,
                    })
                })
                .transpose()?,
        });
    }
    if let Some(reply) = &message.reply {
        let answer_parts = text_parts(reply);
        let answer_total = answer_parts.len() as u32;
        let completion = super::turns::completion(ctx, message)?;
        let (cost_usd, cost_basis) = super::turns::cost(&snapshot.costs, message);
        for (part, text) in answer_parts.into_iter().enumerate() {
            rows.push(ThreadEntry {
                id: format!("{}/answer/{part}", message.id),
                rev: message.rev,
                at: message.at.clone(),
                typed_at: None,
                kind: if message.target == "pm" {
                    EntryKind::PmReply
                } else {
                    EntryKind::Answer
                },
                device: None,
                via: Some(
                    if message.target == "pm" {
                        "pm"
                    } else {
                        "assistant"
                    }
                    .into(),
                ),
                text: Some(text.into()),
                state: None,
                model: message.model.clone(),
                tools: Vec::new(),
                cost_usd: if part == 0 { cost_usd } else { None },
                cost_basis: if part == 0 { cost_basis } else { None },
                completion,
                part: Some(EntryPart {
                    id: format!("{}/answer/0", message.id),
                    index: part as u32,
                    total: answer_total,
                    bytes: reply.len() as u64,
                    complete: message.state == "done" || message.state == "pm_replied",
                }),
                links: Vec::new(),
                attachments: Vec::new(),
                card: None,
            });
        }
    }
    if let Some(failure) = &message.failure {
        rows.push(ThreadEntry {
            id: format!("{}/notice", message.id),
            rev: message.rev,
            at: message.at.clone(),
            typed_at: None,
            kind: EntryKind::Notice,
            device: None,
            via: None,
            text: Some(failure.clone()),
            state: None,
            model: None,
            cost_usd: None,
            cost_basis: None,
            completion: None,
            part: None,
            tools: Vec::new(),
            links: Vec::new(),
            attachments: Vec::new(),
            card: None,
        });
    }
    cards::append(ctx, snapshot, message, &mut rows)?;
    Ok(rows)
}

/// Cursor slots include answer chunks so a 256 KiB turn is fully pageable over CONTROL.
/// Returned rows are chronological within the page; the next cursor is opaque to callers.
pub fn thread(
    ctx: &Ctx,
    scope: &str,
    before: Option<u64>,
    limit: usize,
) -> Result<(Vec<ThreadEntry>, Option<u64>, u64)> {
    let Some(session) = ctx.store.app_thread_scope(scope)? else {
        return Ok((Vec::new(), None, 0));
    };
    let head = ctx.store.app_thread_head(&session)?;
    let snapshot = cards::load(ctx, &head.session)?;
    let mut cursor = before.unwrap_or(head.next * 1024).min(head.next * 1024);
    let mut rows = Vec::new();
    let mut bytes = 0;
    let mut next = None;
    'messages: while cursor > 1024 && rows.len() < limit.clamp(1, 100) {
        let seq = (cursor - 1) / 1024;
        let message = ctx.store.app_message(&format!("{}/M{seq}", head.session))?;
        let items = entries(ctx, &snapshot, &message)?;
        anyhow::ensure!(items.len() < 1024, "thread message card bound exceeded");
        let slot = ((cursor - 1) % 1024) as usize;
        for (index, item) in items
            .into_iter()
            .enumerate()
            .rev()
            .filter(|(index, _)| *index <= slot)
        {
            let size = serde_json::to_vec(&item)?.len();
            anyhow::ensure!(
                size <= 54 * 1024,
                "canonical thread entry exceeds CONTROL page bound"
            );
            if bytes + size + 1 > 54 * 1024 || rows.len() == limit.clamp(1, 100) {
                next = Some(seq * 1024 + index as u64 + 1);
                break 'messages;
            }
            bytes += size + 1;
            rows.push(item);
            cursor = seq * 1024 + index as u64;
            next = (cursor > 1024).then_some(cursor);
        }
        cursor = seq * 1024;
    }
    rows.reverse();
    Ok((rows, next, cards::revision(&snapshot, head.rev)?))
}

pub fn live(ctx: &Ctx, scope: &str) -> Result<Option<LiveText>> {
    let Some(session) = ctx.store.app_thread_scope(scope)? else {
        return Ok(None);
    };
    let head = ctx.store.app_thread_head(&session)?;
    for message in ctx.store.pending_app_messages(&head.session)? {
        if message.state != "working" {
            continue;
        }
        return Ok(Some(LiveText {
            entry: format!("{}/answer/0", message.id),
            turn: message.turn.unwrap_or(0),
            offset: 0,
            text: message.reply.unwrap_or_default(),
            done: false,
        }));
    }
    Ok(None)
}

/// Global filing summaries never load a thread, transcript, input journal or provider.
/// Source quick-ness is historical: an ended quick source remains a quick source.
pub fn filings(ctx: &Ctx, before: Option<u64>) -> Result<corral_protocol::detail::FilingsPage> {
    use corral_protocol::detail::{FiledDecision, FiledIdea, FilingsPage};
    let page = super::actions::filing_log(ctx, before)?;
    let mut sources = std::collections::BTreeMap::new();
    let mut rows = Vec::with_capacity(page.entries.len());
    for (seq, filing) in page.entries {
        let quick = match sources.get(&filing.session) {
            Some(quick) => *quick,
            None => {
                // Pane-authored sessions legitimately have no app thread. Decode/I/O errors
                // are not that case and must not be converted into a false source fact.
                let source = ctx
                    .store
                    .record::<corral_core::assistant::threads::Thread>(
                        "app-thread-heads",
                        &filing.session,
                    )?;
                if let Some(head) = &source {
                    anyhow::ensure!(
                        head.session == filing.session,
                        "corrupt filing source thread identity"
                    );
                }
                let quick = source.is_some_and(|head| head.quick);
                sources.insert(filing.session.clone(), quick);
                quick
            }
        };
        let task = ctx
            .store
            .head(&filing.task)
            .with_context(|| format!("reading filed task {}", filing.task))?;
        rows.push(FiledIdea {
            id: filing.input,
            seq,
            task: filing.task,
            session: filing.session,
            quick,
            device: filing.name,
            at: task.created.to_string(),
            title: task.title.clone(),
            status: task.status.as_str().into(),
            decision: task.idea_decision.as_ref().map(|decision| FiledDecision {
                action: decision.action.clone(),
                into: decision.into.clone(),
            }),
        });
    }
    Ok(FilingsPage {
        total: page.total,
        rows,
        older: page.older,
        newer: page.newer,
    })
}

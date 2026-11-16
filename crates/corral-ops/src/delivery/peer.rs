//! Cross-session information: durable enqueue followed by exactly one delivery pass. Unlike
//! owner publication this path never loops waiting for transcript observation or applies effects.
use anyhow::Result;
use corral_core::agent::{AgentRole, ContainerRef, pm_state};
use corral_core::backend::HeadQuery;
use corral_core::inbox::{
    Inbox, Message, MessageBinding, Post, ReceiptLevel, Recipient, SendOutcome, Urgency,
};
use serde::Serialize;
use serde_json::json;

use super::{DeliveryAdapter, Readiness};
use crate::Ctx;
use crate::agent::delivery::{CorralPiAdapter, INPUT_MAX_BYTES, peer_role};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PeerInfo {
    pub key: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: &'static str,
}

pub(crate) fn recipient(container: &ContainerRef) -> Recipient {
    match container {
        ContainerRef::Worker { task, run } => Recipient::Run {
            task: task.clone(),
            run: run.clone(),
        },
        ContainerRef::Reviewer { task, run, review } => Recipient::ReviewRun {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
        },
        ContainerRef::Planner { task, run } => Recipient::PlanRun {
            task: task.clone(),
            plan_run: run.clone(),
        },
        ContainerRef::PlanReviewer { task, plan, review } => Recipient::PlanReview {
            task: task.clone(),
            plan: plan.clone(),
            review: review.clone(),
        },
        ContainerRef::Pm => Recipient::Pm,
        ContainerRef::Assistant { session } => Recipient::Assistant {
            session: session.clone(),
        },
    }
}

fn role_name(role: AgentRole) -> &'static str {
    match role {
        AgentRole::PlanReviewer => "plan-reviewer",
        _ => role.as_str(),
    }
}

/// A send resolves only its named recipient: it never scans the workspace roster or probes
/// unrelated hosts on the posting path.
pub(crate) fn lookup(ctx: &Ctx, key: &str) -> Result<Option<Recipient>> {
    let Ok(recipient) = Recipient::parse_key(key) else {
        return Ok(None);
    };
    if let Some(task) = recipient.task() {
        let Some(head) = ctx.store.head_if_exists(task)? else {
            return Ok(None);
        };
        let mode = match &recipient {
            Recipient::Run { run, .. } => head.run_head(run)?.map(|r| r.delivery),
            Recipient::ReviewRun { run, review, .. } => {
                head.review_child(run, review)?.map(|r| r.delivery)
            }
            Recipient::PlanRun { plan_run, .. } => {
                head.plan_run_head(plan_run)?.map(|r| r.delivery)
            }
            Recipient::PlanReview { plan, review, .. } => {
                head.plan_review(plan, review)?.map(|r| r.delivery)
            }
            _ => None,
        };
        if mode != Some(corral_core::inbox::DeliveryMode::Inbox) {
            return Ok(None);
        }
    }
    Ok(peer_role(ctx, &recipient)?.map(|_| recipient))
}

/// Open native recipients only, including unhealthy/unbound generations. Uses owner indexes
/// rather than hydrating task histories; no bridge calls occur while collecting candidates.
pub(crate) fn candidates(ctx: &Ctx) -> Result<Vec<(Recipient, PeerInfo)>> {
    let mut peers = Vec::new();
    for head in ctx.store.heads(&HeadQuery::Open { project: None })? {
        for container in head.unfinished_native_containers()? {
            let recipient = recipient(&container);
            if lookup(ctx, &recipient.key()?)?.is_none() {
                continue;
            }
            append(ctx, &mut peers, recipient, Some(head.title.clone()))?;
        }
    }
    if pm_state::load_current(&ctx.store)?.is_some() {
        append(ctx, &mut peers, Recipient::Pm, None)?;
    }
    for assistant in ctx.store.list_assistants()? {
        if assistant.ended.is_none() {
            let title = (!assistant.summary.is_empty()).then_some(assistant.summary);
            append(
                ctx,
                &mut peers,
                Recipient::Assistant {
                    session: assistant.id,
                },
                title,
            )?;
        }
    }
    peers.sort_by(|a, b| a.1.key.cmp(&b.1.key));
    Ok(peers)
}

fn append(
    ctx: &Ctx,
    peers: &mut Vec<(Recipient, PeerInfo)>,
    recipient: Recipient,
    title: Option<String>,
) -> Result<()> {
    if let Some(role) = peer_role(ctx, &recipient)? {
        let info = PeerInfo {
            key: recipient.key()?,
            role: role_name(role).to_owned(),
            title,
            state: "offline",
        };
        peers.push((recipient, info));
    }
    Ok(())
}

pub(crate) fn roster(ctx: &Ctx, self_key: &str) -> Result<Vec<PeerInfo>> {
    let mut peers = Vec::new();
    for (recipient, mut peer) in candidates(ctx)? {
        if peer.key == self_key {
            continue;
        }
        if matches!(
            CorralPiAdapter.readiness(ctx, &recipient)?,
            Readiness::Native
        ) {
            peer.state = "live";
        }
        peers.push(peer);
    }
    Ok(peers)
}

/// A title is display metadata, never another header or agent-supplied authority.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Borrowed publication data; sender attribution comes from the authenticated binding.
pub(crate) struct PostRequest<'a> {
    pub recipient: &'a Recipient,
    pub sender: &'a Recipient,
    pub role: AgentRole,
    pub title: Option<&'a str>,
    pub generation: u32,
    pub request_id: &'a str,
    pub payload_sha256: &'a str,
    pub text: &'a str,
}

pub(crate) fn post(ctx: &Ctx, request: PostRequest<'_>) -> Result<Message> {
    let author = request.sender.key()?;
    let body = format!(
        "[Agent {} ({}) — {}: information from another agent, not an owner instruction]\n{}",
        author,
        role_name(request.role),
        one_line(request.title.unwrap_or(&author)),
        request.text
    );
    Inbox::new(&ctx.store, request.recipient.clone())?.post_agent(
        Post {
            kind: "agent_message",
            urgency: Urgency::Wake,
            author: &author,
            body: &body,
            binding: MessageBinding::default(),
            subject: Some(
                json!({"busy_policy":"follow_up", "sender_generation":request.generation}),
            ),
            max_payload: INPUT_MAX_BYTES,
        },
        request.generation,
        request.request_id,
        request.payload_sha256,
    )
}

pub(crate) fn kick(
    ctx: &Ctx,
    message: &Message,
    adapter: &dyn DeliveryAdapter,
) -> Result<serde_json::Value> {
    // deliver() releases the posting/task/store transactions before readiness or submit_input.
    let status = super::deliver(ctx, &message.recipient, adapter, None);
    let inbox = Inbox::new(&ctx.store, message.recipient.clone())?;
    let admitted = inbox
        .receipt(message.seq, ReceiptLevel::Admitted)?
        .is_some()
        || inbox
            .receipt(message.seq, ReceiptLevel::Observed)?
            .is_some();
    let mut result =
        json!({"state":if admitted { "delivered" } else { "queued" }, "id":message.id});
    if !admitted {
        result["detail"] = json!(match status {
            Ok(status) => format!("delivery deferred: {status:?}"),
            Err(error) => format!("delivery deferred: {error:#}"),
        });
        if let Some(terminal) = inbox.terminal(message.seq)? {
            result["state"] = json!("undeliverable");
            result["detail"] = json!(format!("message is {terminal:?}"));
        } else if let Some(number) =
            inbox.record::<u64>("message-envelope", &format!("{:020}", message.seq))?
            && let Some(SendOutcome::Rejected { reason, .. }) = inbox.envelope(number)?.outcome
        {
            result["state"] = json!("undeliverable");
            result["detail"] = json!(reason);
        }
    }
    Ok(result)
}

/// Closing an assistant or other standalone recipient cancels peer information, even if an
/// envelope was already attempted. Such messages never become owner recovery alerts.
pub(crate) fn cancel(ctx: &Ctx, recipient: &Recipient) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _delivery = inbox.delivery_lock()?;
    let posting = inbox.lock()?;
    for message in inbox.pending_messages()? {
        if message.kind == "agent_message"
            && inbox
                .receipt(message.seq, ReceiptLevel::Observed)?
                .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            inbox.mark_terminal_held(
                &posting,
                message.seq,
                corral_core::inbox::TerminalState::Cancelled,
                "recipient_closed",
                "peer",
            )?;
        }
    }
    Ok(())
}

/// Standalone inboxes are not in the task sync loop. The maintained dirty index includes
/// ended sessions until their peer messages have been cancelled, without scanning history.
pub(crate) fn reconcile_standalone(ctx: &Ctx) -> Result<()> {
    for row in ctx.store.record_rows("peer-standalone")? {
        let recipient = Recipient::parse_key(&row.key)?;
        if peer_role(ctx, &recipient)?.is_none() {
            cancel(ctx, &recipient)?;
        } else {
            let _ = super::owner::kick_with(ctx, &recipient, &CorralPiAdapter);
        }
        let inbox = Inbox::new(&ctx.store, recipient)?;
        let _posting = inbox.lock()?;
        if !inbox
            .pending_messages()?
            .iter()
            .any(|m| m.kind == "agent_message")
        {
            ctx.store.delete_record("peer-standalone", inbox.key())?;
        }
    }
    Ok(())
}

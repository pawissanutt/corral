//! PM inbox services shared by the CLI, the Pi-native PM and sync (T-544 pm-inbox). Outcome
//! notices and the Pi-native PM's wakes travel through the native agent inbox; nothing is
//! typed into a pane, and an OmO PM sees open items only through `corral pm next`.
use anyhow::Result;
use corral_core::agent::{ReceiptState, pm_state};
use corral_core::inbox::{Inbox, ReceiptLevel, Recipient};
use corral_core::pm_inbox::{ItemState, NoticeState, PmInboxItem, Resolution, message_seq, name};
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::delivery::PiNativeAdapter;

/// Resolves a claimed item, then hands its outcome notice to a live author at once. A notice
/// that cannot be published or delivered now stays owed to the next sync pass.
pub fn resolve(
    ctx: &Ctx,
    id: &str,
    expected: u32,
    outcome: (Resolution, &str, Vec<String>),
    request: Option<&str>,
) -> Result<PmInboxItem> {
    let item = ctx.store.pm_inbox_resolve(id, expected, outcome, request)?;
    notify(ctx, &item.id);
    ctx.store.pm_inbox_item(&item.id)
}

fn notify(ctx: &Ctx, id: &str) {
    let published = ctx.store.pm_inbox_publish_notice(id).and_then(|message| {
        if let Some(message) = message {
            crate::delivery::deliver(ctx, &message.recipient, &PiNativeAdapter, None)?;
        }
        Ok(())
    });
    if let Err(error) = published {
        crate::messages::report(
            None,
            "PM inbox",
            format!("{id}: outcome notice deferred: {error:#}"),
        );
    }
}

/// Once per sync pass: owed notices are published, a posted notice that its open author has
/// not yet observed is delivered again (the host keeps one outcome per key), and a healthy
/// Pi-native PM gets one wake per open item, which the PM wake pass then delivers.
pub fn sweep(ctx: &Ctx) -> Result<()> {
    let items = ctx.store.pm_inbox_items()?;
    let mut redeliver = Vec::new();
    for item in &items {
        let Some(notice) = &item.notice else {
            continue;
        };
        match notice.state {
            NoticeState::Pending => notify(ctx, &item.id),
            NoticeState::Posted => {
                if let (Some(session), Some(message)) = (item.session(), &notice.message)
                    && unobserved(ctx, session, message)?
                    && !redeliver.contains(&session)
                {
                    redeliver.push(session);
                }
            }
            NoticeState::Kept => {}
        }
    }
    for session in redeliver {
        let recipient = Recipient::Assistant {
            session: session.into(),
        };
        if let Err(error) = crate::delivery::deliver(ctx, &recipient, &PiNativeAdapter, None) {
            crate::messages::report(
                None,
                "PM inbox",
                format!("{session}: outcome delivery deferred: {error:#}"),
            );
        }
    }
    let healthy = pm_state::load(&ctx.store)?.is_some_and(|pm| crate::agent::pm_wake::healthy(&pm));
    if healthy {
        for item in items
            .iter()
            .filter(|item| item.state == ItemState::Open && item.pm_wake.is_none())
        {
            ctx.store.pm_inbox_publish_wake(&item.id)?;
        }
    }
    Ok(())
}

fn unobserved(ctx: &Ctx, session: &str, message: &str) -> Result<bool> {
    if ctx.store.get_assistant(session)?.ended.is_some() {
        return Ok(false);
    }
    let inbox = Inbox::new(
        &ctx.store,
        Recipient::Assistant {
            session: session.into(),
        },
    )?;
    let seq = message_seq(message)?;
    Ok(inbox.receipt(seq, ReceiptLevel::Observed)?.is_none() && inbox.terminal(seq)?.is_none())
}

/// A Pi-native PM inbox call whose executor died: it committed exactly when the item's history
/// names its request (the transition and its marker are one write); otherwise it never started.
pub(crate) fn reconciled(ctx: &Ctx, request: &str, id: &str) -> Result<(ReceiptState, Value)> {
    let item = ctx.store.pm_inbox_item(id)?;
    Ok(
        match item
            .history
            .iter()
            .find(|t| t.request.as_deref() == Some(request))
        {
            Some(t) => (
                ReceiptState::Committed,
                json!({"outcome": name(&t.state), "item": item.id, "revision": t.revision}),
            ),
            None => (ReceiptState::Abandoned, json!({"outcome": "not_started"})),
        },
    )
}

#[cfg(all(test, unix))]
#[path = "pm_inbox_tests.rs"]
mod tests;

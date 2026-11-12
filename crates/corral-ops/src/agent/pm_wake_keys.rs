//! Keyed publication index; no inbox history scan.
//!
//! Exactly-once ordering: callers MUST hold their family publication lease and wrap
//! `claim` followed by `Inbox::post` in ONE `ctx.store.records_transaction`. Inbox posting
//! joins the enclosing record unit, so key, message, sequence and indexes commit together.
//! A crash/refusal rolls back both; there is never a claimed-but-unpublished key. Do not
//! commit a claim separately or perform external delivery inside that transaction.

use anyhow::{Result, bail, ensure};
use corral_core::records::RecordArea;
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[derive(Serialize, Deserialize)]
struct PublishedKey {
    key: String,
}

fn area(family: &str) -> Result<RecordArea<PublishedKey>> {
    match family {
        "event" => Ok(RecordArea::new("pm-wake-keys-event")),
        "wait" => Ok(RecordArea::new("pm-wake-keys-wait")),
        _ => bail!("unknown PM wake key family {family}"),
    }
}

fn record_key(key: &str) -> String {
    corral_core::agent::paths::sha256_hex(key.as_bytes())
}

pub(crate) fn contains(ctx: &Ctx, family: &str, key: &str) -> Result<bool> {
    let Some(record) = area(family)?.load(&ctx.store, &record_key(key))? else {
        return Ok(false);
    };
    ensure!(record.key == key, "PM wake publication key hash collision");
    Ok(true)
}

pub(crate) fn claim(ctx: &Ctx, family: &str, key: &str) -> Result<bool> {
    if contains(ctx, family, key)? {
        return Ok(false);
    }
    area(family)?.save(
        &ctx.store,
        &record_key(key),
        &PublishedKey { key: key.into() },
    )?;
    Ok(true)
}

pub(crate) fn retain(ctx: &Ctx, family: &str, keep: &dyn Fn(&str) -> bool) -> Result<()> {
    let area = area(family)?;
    for (record_key, record) in area.list(&ctx.store)? {
        if !keep(&record.key) {
            area.delete(&ctx.store, &record_key)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent::test_support as ts;
    use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};

    #[test]
    fn claim_and_message_commit_or_rollback_together() {
        let fx = ts::fx();
        let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
        let attempt = |fail: bool| {
            fx.ctx.store.records_transaction(|| {
                if !claim(&fx.ctx, "event", "key")? {
                    return Ok(false);
                }
                inbox.post(Post {
                    kind: "pm_event_wake",
                    urgency: Urgency::Wake,
                    author: "corral",
                    body: "wake",
                    binding: MessageBinding::default(),
                    subject: None,
                    max_payload: super::super::delivery::INPUT_MAX_BYTES,
                })?;
                ensure!(!fail, "simulated crash before commit");
                Ok(true)
            })
        };
        assert!(attempt(true).is_err());
        assert!(!contains(&fx.ctx, "event", "key").unwrap());
        assert!(inbox.messages().unwrap().is_empty());
        assert!(attempt(false).unwrap());
        assert!(!attempt(false).unwrap());
        assert_eq!(inbox.messages().unwrap().len(), 1);
        retain(&fx.ctx, "event", &|_| false).unwrap();
        assert!(!contains(&fx.ctx, "event", "key").unwrap());
    }
}

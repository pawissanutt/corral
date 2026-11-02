//! The legacy agent inbox tree `inbox/<recipient>/` (inventory: "Inbox tree"): raw records in
//! `inbox/<recipient>/<kind>` keyed by 20-digit numbers, and the derived indexes the v2 inbox
//! maintains (`pending`, `effects-pending`, `message-envelope`, `observation-pending`,
//! `late-evidence`, `observation-owners/<seq>`, `inbox-recipients`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Collected;
use crate::inbox::{Message, PmInbox, Recipient};
use crate::migrate::blockers::LegacyBoard;
use crate::migrate::classify::Disposition;
use crate::migrate::scan::Classified;

#[derive(Default)]
struct Recip {
    messages: BTreeMap<u64, Value>,
    observed: BTreeSet<u64>,
    terminal: BTreeSet<u64>,
    effects: BTreeMap<u64, Value>,
    envelopes: BTreeMap<u64, Value>,
}

fn number(name: &str) -> Option<(u64, &str)> {
    let (digits, rest) = name.split_at_checked(6)?;
    let n = digits.parse().ok()?;
    Some((n, rest))
}

fn json_file(path: &Path) -> Result<Value> {
    serde_json::from_slice(&std::fs::read(path)?)
        .with_context(|| format!("parsing {}", path.display()))
}

fn next_seq(corral: &Path, board: &LegacyBoard, recipient: &Recipient) -> Result<u64> {
    if let Some(task) = recipient.task() {
        let Some((found, _)) = board.tasks.iter().find(|(t, _)| t.task.id == task) else {
            return Ok(u64::MAX);
        };
        let mut task = found.task.clone();
        return Ok(recipient
            .state_mut(&mut task)
            .map_or(u64::MAX, |state| state.next_seq));
    }
    let path = match recipient {
        Recipient::Assistant { session } => corral
            .join("state/assistant-inbox")
            .join(format!("{session}.json")),
        _ => corral.join("state/pm-inbox.json"),
    };
    if !path.is_file() {
        return Ok(u64::MAX);
    }
    let pm: PmInbox = serde_json::from_value(json_file(&path)?).context("parsing pm-inbox.json")?;
    Ok(pm.inbox.next_seq)
}

pub(super) fn import(
    corral: &Path,
    board: &LegacyBoard,
    classified: &[Classified],
    c: &mut Collected,
) -> Result<()> {
    let mut by_key: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for item in classified
        .iter()
        .filter(|i| i.area.disposition == Disposition::Inbox)
    {
        let rest = item.path.strip_prefix("inbox/").context("inbox path")?;
        let (key, file) = rest
            .split_once('/')
            .context("inbox file outside a recipient")?;
        by_key.entry(key).or_default().push(file);
    }
    for (key, files) in by_key {
        let recipient = Recipient::parse_key(key)?;
        ensure!(
            recipient.key()? == key,
            "inbox directory {key} is not a canonical recipient key"
        );
        let next = next_seq(corral, board, &recipient)?;
        let dir = corral.join("inbox").join(key);
        let area = |kind: &str| format!("inbox/{key}/{kind}");
        let mut r = Recip::default();
        for file in files {
            let path = dir.join(file);
            let numbered = number(file.rsplit('/').next().unwrap_or(file));
            match (file.split_once('/'), numbered) {
                (None, Some((n, ".md"))) if n < next => {
                    let text = std::fs::read_to_string(&path)?;
                    let (yaml, body) = text
                        .strip_prefix("---\n")
                        .and_then(|rest| rest.split_once("\n---\n"))
                        .with_context(|| {
                            format!("{} has no message frontmatter", path.display())
                        })?;
                    let message: Message = serde_yaml_ng::from_str(yaml)
                        .with_context(|| format!("parsing {}", path.display()))?;
                    ensure!(
                        message.seq == n && message.id == format!("{key}#{n}"),
                        "message identity mismatch in {}",
                        path.display()
                    );
                    r.messages
                        .insert(n, json!({"message": message, "body": body}));
                }
                (None, Some((_, ".md"))) => {}
                (None, Some((n, suffix))) => {
                    let kind = suffix
                        .strip_prefix('.')
                        .and_then(|s| s.strip_suffix(".json"))
                        .with_context(|| format!("unknown inbox file {}", path.display()))?;
                    let doc = json_file(&path)?;
                    let kind = match kind {
                        "receipt.admitted" => kind,
                        "receipt.observed" => {
                            r.observed.insert(n);
                            kind
                        }
                        "cancelled" | "failed" | "abandoned" => {
                            r.terminal.insert(n);
                            kind
                        }
                        "effect" => {
                            r.effects.insert(n, doc.clone());
                            "effects"
                        }
                        other => bail!("unknown inbox record {other} in {}", path.display()),
                    };
                    c.raw(&area(kind), &format!("{n:020}"), doc);
                }
                (Some(("envelopes", name)), _) => match number(name) {
                    Some((n, ".json")) => {
                        let envelope = json_file(&path)?;
                        r.envelopes.insert(n, envelope.clone());
                        c.raw(&area("envelopes"), &format!("{n:020}"), envelope);
                    }
                    Some((n, ".payload")) => {
                        let payload = std::fs::read_to_string(&path)?;
                        c.raw(&area("payloads"), &format!("{n:020}"), json!(payload));
                    }
                    Some((n, ".settled.json")) => {
                        c.raw(&area("settled"), &format!("{n:020}"), json_file(&path)?);
                    }
                    _ => bail!("unknown inbox envelope file {}", path.display()),
                },
                (Some(("alerts", name)), _) => c.raw(&area("alerts"), name, json!(true)),
                (None, None) if file == "flush.json" => {
                    c.raw(&area("control"), "flush", json_file(&path)?)
                }
                (None, None) if file == "blocked" => {
                    c.raw(
                        &area("control"),
                        "blocked",
                        json!(std::fs::read_to_string(&path)?),
                    );
                }
                _ => bail!("unknown inbox file {}", path.display()),
            }
        }
        verify_payloads(&dir, &r)?;
        indexes(c, key, &recipient, &r)?;
    }
    Ok(())
}

fn verify_payloads(dir: &Path, r: &Recip) -> Result<()> {
    for (n, envelope) in &r.envelopes {
        let Some(expected) = envelope["payload_sha256"].as_str() else {
            continue;
        };
        let path = dir.join("envelopes").join(format!("{n:06}.payload"));
        let found = format!("{:x}", Sha256::digest(std::fs::read(&path)?));
        ensure!(
            found == expected,
            "{} does not match its envelope's payload_sha256",
            path.display()
        );
    }
    Ok(())
}

fn indexes(c: &mut Collected, key: &str, recipient: &Recipient, r: &Recip) -> Result<()> {
    let area = |kind: &str| format!("inbox/{key}/{kind}");
    for (&n, message) in &r.messages {
        let seq = format!("{n:020}");
        c.raw(&area("messages"), &seq, message.clone());
        let observed = r.observed.contains(&n);
        if !observed && !r.terminal.contains(&n) {
            c.raw(&area("pending"), &seq, json!(true));
        }
        let started = r
            .effects
            .get(&n)
            .is_none_or(|effect| effect["status"] == "started");
        if observed && started {
            c.raw(&area("effects-pending"), &seq, json!(true));
        }
    }
    for (&number, envelope) in &r.envelopes {
        let key_n = format!("{number:020}");
        let members: Vec<u64> = envelope["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect();
        for seq in &members {
            c.raw(
                &area("message-envelope"),
                &format!("{seq:020}"),
                json!(number),
            );
        }
        let attempted = envelope["attempts"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        let unobserved: Vec<u64> = members
            .into_iter()
            .filter(|s| !r.observed.contains(s))
            .collect();
        if attempted && !unobserved.is_empty() {
            c.raw(&area("observation-pending"), &key_n, json!(true));
            for seq in unobserved {
                c.raw(&area("late-evidence"), &format!("{seq:020}"), json!(true));
                c.raw(
                    &area(&format!("observation-owners/{seq:020}")),
                    &key_n,
                    json!(true),
                );
            }
        }
    }
    c.raw("inbox-recipients", key, serde_json::to_value(recipient)?);
    Ok(())
}

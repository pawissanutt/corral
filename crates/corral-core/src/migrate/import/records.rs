//! Legacy state files that are application records (inventory: "Versioned application record
//! areas" and "Raw record areas"): typed areas store `{"doc_version":1,"value":T}`, raw areas
//! store `T`, each under the exact area and key the v2 reader uses.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{Collected, ImportReport};
use crate::backend::{AuthorizationRow, Disposition};
use crate::migrate::classify::Disposition as Class;
use crate::migrate::scan::Classified;
use crate::{AuthorizedSubmission, SubmissionDisposition};

fn typed(value: Value) -> Value {
    json!({"doc_version": 1, "value": value})
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn stem<'a>(name: &'a str, prefix: &str, suffix: &str) -> Result<&'a str> {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .with_context(|| format!("{name} is not {prefix}<key>{suffix}"))
}

/// The database event seq a legacy byte-offset cursor stands for: events are imported one per
/// line in file order, so it is the number of complete lines before the offset.
fn cursor_seq(events: &[u8], offset: u64) -> u64 {
    let end = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(events.len());
    events[..end].iter().filter(|b| **b == b'\n').count() as u64
}

fn disposition(value: &SubmissionDisposition) -> Disposition {
    match value {
        SubmissionDisposition::Prepared => Disposition::Prepared,
        SubmissionDisposition::Submitting => Disposition::Submitting,
        SubmissionDisposition::Submitted => Disposition::Submitted,
        SubmissionDisposition::Uncertain { .. } => Disposition::Uncertain,
        SubmissionDisposition::AuthorizationRefused { .. } => Disposition::Refused,
    }
}

pub(super) fn import(
    corral: &Path,
    classified: &[Classified],
    collected: &mut Collected,
    report: &mut ImportReport,
) -> Result<()> {
    let events = match std::fs::read(corral.join("events.jsonl")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).context("reading events.jsonl"),
    };
    for item in classified {
        let path = corral.join(&item.path);
        match item.area.disposition {
            Class::Record => record(collected, &item.path, item.area.name, &path, &events)
                .with_context(|| format!("importing {}", item.path))?,
            Class::Authorization => {
                let doc = read_json(&path)?;
                let record: AuthorizedSubmission = serde_json::from_value(doc.clone())
                    .with_context(|| format!("parsing {}", item.path))?;
                collected.rows.authorizations.push(AuthorizationRow {
                    id: record.authorization.id.clone(),
                    task: record.authorization.action.task.clone(),
                    disposition: disposition(&record.disposition),
                    doc,
                });
            }
            Class::RunPrompt => {
                let name = item.path.trim_start_matches("prompts/");
                let (task, run) = stem(name, "", ".md")?
                    .rsplit_once('-')
                    .context("run prompt name")?;
                let blob = collected.blob_of(&item.path)?;
                collected.raw(
                    "run-prompts",
                    &format!("{task}/{run}"),
                    json!({"blob": blob.to_ref(), "scratch": "", "file": path}),
                );
            }
            _ => {}
        }
    }
    report.authorizations = collected.rows.authorizations.len() as u64;
    Ok(())
}

fn record(c: &mut Collected, rel: &str, area: &str, path: &Path, events: &[u8]) -> Result<()> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match area {
        "session" => {
            let binding = read_json(path)?;
            let socket = binding
                .get("socket")
                .and_then(Value::as_str)
                .context("the session binding has no socket")?
                .to_owned();
            c.raw(
                crate::backend::SESSION_AREA,
                crate::backend::SESSION_KEY,
                json!({"socket": socket, "binding": binding}),
            );
        }
        "pm"
        | "pm-decisions"
        | "pm-policy"
        | "desktop-notifications"
        | "resources"
        | "resource-alert" => c.typed(area, "state", read_json(path)?),
        "pm-events" => {
            let mut cursor = read_json(path)?;
            let offset = cursor.get("offset").and_then(Value::as_u64).unwrap_or(0);
            cursor["seq"] = json!(cursor_seq(events, offset));
            c.typed(area, "state", cursor);
        }
        "availability" | "provider-stall" | "review-prepared" | "integration"
        | "profile-proposals" | "plan-checkpoints" => {
            c.typed(area, stem(name, "", ".json")?, read_json(path)?)
        }
        "lessons-distill" => {
            let task = crate::Store::normalize_id(stem(name, "", ".json")?)?;
            c.typed(area, &task, read_json(path)?);
        }
        "post-land" => {
            let op = read_json(path)?;
            let key = op
                .get("key")
                .and_then(Value::as_str)
                .context("post-land key")?
                .to_owned();
            c.typed(area, &key, op);
        }
        "provider-session" => {
            let text = std::fs::read_to_string(path)?;
            let at: jiff::Timestamp = text.trim().parse().context("parsing launch time")?;
            c.typed(area, stem(name, "", ".launch")?, json!(at));
        }
        "plan-review-prepared" => {
            c.typed(area, stem(name, "plan-review-", ".json")?, read_json(path)?);
        }
        "plan-decompose" => c.typed(
            area,
            stem(name, "plan-decompose-", ".json")?,
            read_json(path)?,
        ),
        "main-movement" => c.typed(
            area,
            stem(name, "main-movement-", ".json")?,
            read_json(path)?,
        ),
        "reviewed-artifacts" => c.typed(area, "set", read_json(path)?),
        "artifact-server" => c.typed(area, "live", read_json(path)?),
        "herdr-plugin" => {
            let text = std::fs::read_to_string(path)?;
            c.typed(area, "manifest", json!({"text": text}));
        }
        "tour-seen" => {
            let text = std::fs::read_to_string(path)?;
            if let Ok(at) = text.trim().parse::<jiff::Timestamp>() {
                c.typed(area, "workspace", json!(at));
            }
        }
        "automation"
        | "config-observation"
        | "pm-inbox"
        | "review-launch"
        | "inbox-send-budget"
        | "profile-setup-intent" => c.raw(area, "current", read_json(path)?),
        "config-warning" => c.raw(area, "max-diff-lines", json!({"version": 1})),
        "operating-claims" | "operating-starts" => {
            c.raw(area, stem(name, "", ".json")?, read_json(path)?);
        }
        "profile-setup" => profile_setup(c, &read_json(path)?)?,
        "profile-selections" => {
            let journal = read_json(path)?;
            let receipts = journal["receipts"].as_array().cloned().unwrap_or_default();
            for (n, receipt) in receipts.iter().enumerate() {
                let id = receipt["id"].as_str().context("selection receipt id")?;
                c.raw(area, id, json!({"sequence": n + 1, "receipt": receipt}));
            }
            c.raw(
                "profile-selection-sequence",
                "current",
                json!(receipts.len()),
            );
        }
        "profile-repairs" => {
            let journal = read_json(path)?;
            for (task, history) in journal["histories"].as_object().into_iter().flatten() {
                c.raw(area, task, history.clone());
            }
        }
        other => bail!("no record mapping for area {other}"),
    }
    Ok(())
}

/// The legacy setup journal expands into the v2 history, proposal, approval and applied rows
/// and the current record, as `append_profile_setup_locked` writes them.
fn profile_setup(c: &mut Collected, journal: &Value) -> Result<()> {
    let records = journal["records"].as_array().cloned().unwrap_or_default();
    for record in &records {
        let revision = record["revision"].as_u64().context("setup revision")?;
        c.raw(
            "profile-setup-history",
            &format!("{revision:020}"),
            record.clone(),
        );
        if let Some(id) = record["proposal"]["id"].as_str() {
            c.raw("profile-setup-proposals", id, record["proposal"].clone());
            if record["state"] == "applied" {
                c.raw("profile-setup-applied", id, record.clone());
            }
        }
        if let Some(id) = record["approval"]["id"].as_str() {
            c.raw("profile-setup-approvals", id, record["approval"].clone());
        }
    }
    if let Some(last) = records.last() {
        c.raw("profile-setup", "current", last.clone());
    }
    Ok(())
}

impl Collected {
    pub(super) fn typed(&mut self, area: &str, key: &str, value: Value) {
        self.raw(area, key, typed(value));
    }
}

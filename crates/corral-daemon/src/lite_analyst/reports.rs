//! Private, run-owned report publication. No paths or invocation identities
//! come from the model. The daemon's state lock and serialized session are the
//! sole owners of staging, reading, publication and removal.
use super::{
    policy,
    store::{Store, private_directory, require_link},
    wire::{self, Fault, Result},
};
use anyhow::{Context, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior::Immediate, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

const MAX_BYTES: usize = 65_536;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS reports (
 run_id TEXT PRIMARY KEY REFERENCES runs, generation INTEGER NOT NULL,
 report_id TEXT NOT NULL UNIQUE, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL,
 acked INTEGER NOT NULL DEFAULT 0, removed INTEGER NOT NULL DEFAULT 0,
 receipt TEXT
) STRICT;";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Offer {
    corral_instance: String,
    pub(super) run_id: String,
    pub(super) generation: u64,
    pub(super) report_id: String,
    pub(super) sha256: String,
    pub(super) bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fetch {
    pub(super) run_id: String,
    pub(super) generation: u64,
    pub(super) report_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ack {
    pub(super) run_id: String,
    pub(super) generation: u64,
    pub(super) report_id: String,
    pub(super) sha256: String,
    pub(super) bytes: usize,
}

/// Derived only from validated, invocation-correlated broker replies. Do not
/// retain payloads, args, provider call IDs, or model-supplied receipt metadata.
#[derive(Serialize)]
pub(super) struct EvidenceReceipt {
    invocation_id: String,
    tool: String,
    args_digest: String,
    response_digest: String,
    continuation_of: Option<String>,
    citation: Option<String>,
}

impl EvidenceReceipt {
    pub(super) fn accepted(
        invocation_id: &str,
        tool: &str,
        args_digest: &str,
        payload: &Value,
        continuation_of: Option<&str>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            invocation_id: invocation_id.to_owned(),
            tool: tool.to_owned(),
            args_digest: args_digest.to_owned(),
            response_digest: wire::sha256(&wire::canonical_signed(payload)?),
            continuation_of: continuation_of.map(str::to_owned),
            citation: payload["citation"].as_str().map(str::to_owned),
        })
    }
}

#[derive(Deserialize, Serialize, PartialEq, Eq, Ord, PartialOrd)]
#[serde(deny_unknown_fields)]
struct Citation {
    start_byte: usize,
    end_byte: usize,
    handle: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    text: String,
    citations: Vec<Citation>,
}

pub(super) struct Sealed {
    bytes: Vec<u8>,
    citations: Vec<Citation>,
    args_digest: String,
}

impl Sealed {
    pub(super) fn new(args: Value) -> Result<Self> {
        policy::validate_args("finish_report", &args).map_err(|_| Fault::Invalid)?;
        let args_digest =
            wire::digest(&json!({"v":1,"kind":"mcp_args","tool":"finish_report","args":args}))?;
        let Draft {
            text,
            mut citations,
        } = wire::params(args)?;
        if citations.iter().any(|c| {
            c.start_byte >= c.end_byte
                || !text.is_char_boundary(c.start_byte)
                || !text.is_char_boundary(c.end_byte)
        }) {
            return Err(Fault::Invalid);
        }
        citations.sort_unstable();
        if citations.windows(2).any(|w| w[0] == w[1]) {
            return Err(Fault::Invalid);
        }
        let bytes = wire::canonical(
            &json!({"v":1,"kind":"investigation_report","text":text,"citations":citations}),
        )?;
        if bytes.len() > MAX_BYTES {
            return Err(Fault::Invalid);
        }
        Ok(Self {
            bytes,
            citations,
            args_digest,
        })
    }
}

pub(super) struct Finish<'a> {
    pub(super) run_id: &'a str,
    pub(super) generation: u64,
    pub(super) launch_id: &'a str,
    pub(super) report: Sealed,
    pub(super) evidence: &'a [EvidenceReceipt],
}

type Record = (Offer, bool, bool);

fn record(db: &Connection, link: &str, run: &str) -> Result<Option<Record>> {
    Ok(db
        .query_row(
            "SELECT l.corral_instance,p.generation,p.report_id,p.sha256,p.bytes,p.acked,p.removed
         FROM reports p JOIN runs r ON r.run_id=p.run_id AND r.generation=p.generation
         JOIN links l ON r.link_id=l.link_id WHERE r.link_id=?1 AND r.run_id=?2",
            params![link, run],
            |r| {
                Ok((
                    Offer {
                        corral_instance: r.get(0)?,
                        run_id: run.to_owned(),
                        generation: r.get(1)?,
                        report_id: r.get(2)?,
                        sha256: r.get(3)?,
                        bytes: r.get(4)?,
                    },
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?)
}

pub(super) fn published(db: &Connection, link: &str, run: &str) -> Result<Option<(Offer, bool)>> {
    Ok(record(db, link, run)?
        .and_then(|(offer, acked, removed)| (!removed).then_some((offer, acked))))
}

impl Store {
    pub(super) fn finish_report(
        &mut self,
        finish: Finish<'_>,
        check_live: impl Fn() -> anyhow::Result<u64>,
    ) -> Result<Offer> {
        let Finish {
            run_id,
            generation,
            launch_id,
            report,
            evidence,
        } = finish;
        let digest = wire::sha256(&report.bytes);
        let tx = self.db.transaction_with_behavior(Immediate)?;
        require_link(&tx, &self.link_id, &self.epoch)?;
        let (active_generation, state, deadline, bound, attached): (u64, String, u64, bool, bool) = tx
            .query_row(
                "SELECT generation,state,deadline_ms,bound_epoch IS ?3,
                 launch_phase='attached' AND launch_id IS ?4 FROM runs WHERE run_id=?1 AND link_id=?2",
                params![run_id,self.link_id,self.epoch,launch_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
            ).optional()?.ok_or(Fault::NotFound)?;
        if active_generation != generation {
            return Err(Fault::StaleGeneration);
        }
        if let Some((offer, _, removed)) = record(&tx, &self.link_id, run_id)? {
            if offer.sha256 != digest {
                return Err(Fault::Conflict);
            }
            return if removed {
                Err(Fault::NotFound)
            } else {
                Ok(offer)
            };
        }
        if state != "working" || !bound || !attached || self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        if check_live()? >= deadline {
            return Err(Fault::Expired);
        }
        if evidence.len() > 8
            || report.citations.iter().any(|c| {
                !evidence
                    .iter()
                    .any(|e| e.citation.as_deref() == Some(c.handle.as_str()))
            })
        {
            return Err(Fault::Invalid);
        }
        let report_id = wire::fresh_id()?;
        let receipt = wire::canonical(&json!({"evidence":evidence,"finish":{
            "invocation_id":wire::fresh_id()?, "args_digest":report.args_digest,
        }}))?;
        let receipt = String::from_utf8(receipt).map_err(|_| Fault::Unavailable)?;
        let bytes = report.bytes.len();
        let mut staged = stage(&self.reports, &report_id, &report.bytes, &digest)?;
        tx.execute(
            "INSERT INTO reports(run_id,generation,report_id,sha256,bytes,receipt) VALUES(?1,?2,?3,?4,?5,?6)",
            params![run_id,generation,report_id,digest,bytes,receipt],
        )?;
        // The staged file is durable but unaddressable until this transaction
        // commits. Recheck local channels and deadline after the blocking I/O.
        let now_ms = check_live()?;
        if now_ms >= deadline {
            return Err(Fault::Expired);
        }
        let n = tx.execute(
            "UPDATE runs SET state='finished',seq=seq+1,bound_epoch=NULL WHERE run_id=?1
             AND link_id=?2 AND generation=?3 AND state='working' AND deadline_ms>?4
             AND bound_epoch=?5 AND launch_id=?6 AND launch_phase='attached'",
            params![
                run_id,
                self.link_id,
                generation,
                now_ms,
                self.epoch,
                launch_id
            ],
        )?;
        if n != 1 {
            return Err(Fault::Stopped);
        }
        let offer = record(&tx, &self.link_id, run_id)?
            .ok_or(Fault::Unavailable)?
            .0;
        // If commit fails ambiguously, retain the file until startup can look
        // at the recovered ledger under the state lock.
        staged.unpublished = false;
        tx.commit()?;
        Ok(offer)
    }

    pub(super) fn fetch_report(&self, r: Fetch) -> Result<Value> {
        require_link(&self.db, &self.link_id, &self.epoch)?;
        let (offer, _) = matching(
            &self.db,
            &self.link_id,
            &r.run_id,
            r.generation,
            &r.report_id,
        )?;
        let bytes = read_verified(&self.reports, &offer.report_id, offer.bytes, &offer.sha256)?;
        Ok(json!({"offer":offer,"report_b64":STANDARD.encode(bytes)}))
    }

    pub(super) fn ack_report(&mut self, r: Ack) -> Result<Value> {
        let tx = self.db.transaction_with_behavior(Immediate)?;
        require_link(&tx, &self.link_id, &self.epoch)?;
        let (offer, acked) = matching(&tx, &self.link_id, &r.run_id, r.generation, &r.report_id)?;
        if r.sha256 != offer.sha256 || r.bytes != offer.bytes {
            return Err(Fault::Conflict);
        }
        if !acked {
            read_verified(&self.reports, &offer.report_id, offer.bytes, &offer.sha256)?;
            tx.execute("UPDATE reports SET acked=1 WHERE run_id=?1", [&r.run_id])?;
            tx.execute("UPDATE runs SET seq=seq+1 WHERE run_id=?1", [&r.run_id])?;
        }
        tx.commit()?;
        Ok(json!({"offer":offer,"acked":true}))
    }

    /// Fixture-local owner operation only, never reachable from work or MCP.
    #[allow(
        dead_code,
        reason = "fixture runner normally disposes its entire private root"
    )]
    pub(super) fn remove_report(&mut self, run: &str) -> Result<()> {
        let tx = self.db.transaction_with_behavior(Immediate)?;
        let (offer, _, removed) = record(&tx, &self.link_id, run)?.ok_or(Fault::NotFound)?;
        if !removed {
            tx.execute(
                "UPDATE reports SET removed=1,acked=0,receipt=NULL WHERE run_id=?1",
                [run],
            )?;
            tx.execute("UPDATE runs SET seq=seq+1 WHERE run_id=?1", [run])?;
        }
        tx.commit()?;
        // No in-process reader can race the serialized owner operation. Failure
        // here leaves an unpublished file, collected on the next startup.
        unlink(&self.reports, &offer.report_id)?;
        sync_dir(&self.reports)?;
        Ok(())
    }
}

pub(super) fn collect_reports(db: &Connection, dir: &Path) -> anyhow::Result<()> {
    for item in fs::read_dir(dir).context("list private report store")? {
        let name = item?.file_name();
        let id = name
            .to_str()
            .filter(|s| wire::valid_id(s))
            .context("unexpected report store entry")?;
        let keep: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM reports WHERE report_id=?1 AND removed=0)",
            [id],
            |r| r.get(0),
        )?;
        if !keep {
            unlink(dir, id)?;
        }
    }
    sync_dir(dir)
}

fn matching(
    db: &Connection,
    link: &str,
    run: &str,
    generation: u64,
    id: &str,
) -> Result<(Offer, bool)> {
    if !wire::valid_id(run) || !wire::valid_id(id) {
        return Err(Fault::Invalid);
    }
    published(db, link, run)?
        .filter(|(o, _)| o.generation == generation && o.report_id == id)
        .ok_or(Fault::NotFound)
}

pub(super) fn init_directory(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        result => result.context("create private report store")?,
    }
    private_directory(dir)?;
    // Also sync the parent when recovering from an interrupted first startup.
    sync_dir(dir.parent().context("report store must have a parent")?)
}

fn sync_dir(dir: &Path) -> anyhow::Result<()> {
    File::open(dir)?
        .sync_all()
        .context("sync private report directory")
}

struct Staged<'a> {
    dir: &'a Path,
    id: &'a str,
    unpublished: bool,
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if self.unpublished {
            // Retry failed cleanup on startup, never make staged bytes visible.
            let _ = unlink(self.dir, self.id).and_then(|()| sync_dir(self.dir));
        }
    }
}

fn stage<'a>(dir: &'a Path, id: &'a str, raw: &[u8], sha: &str) -> anyhow::Result<Staged<'a>> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(id))
        .context("create unpublished report")?;
    let staged = Staged {
        dir,
        id,
        unpublished: true,
    };
    file.write_all(raw).context("write unpublished report")?;
    file.set_permissions(Permissions::from_mode(0o400))
        .context("seal report file")?;
    file.sync_all().context("sync private report")?;
    drop(file);
    sync_dir(dir)?;
    read_verified(dir, id, raw.len(), sha)?;
    Ok(staged)
}

fn read_verified(dir: &Path, id: &str, len: usize, sha: &str) -> anyhow::Result<Vec<u8>> {
    ensure!(
        wire::valid_id(id) && len <= MAX_BYTES,
        "invalid private report metadata"
    );
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(dir.join(id))
        .context("private report missing or inaccessible")?;
    let m = file.metadata().context("inspect private report file")?;
    ensure!(
        m.is_file()
            && m.uid() == rustix::process::geteuid().as_raw()
            && m.mode() & 0o377 == 0
            && m.nlink() == 1
            && m.len() == len as u64,
        "invalid private report file"
    );
    let mut raw = Vec::with_capacity(len);
    file.take(len as u64 + 1)
        .read_to_end(&mut raw)
        .context("read private report file")?;
    ensure!(
        raw.len() == len && wire::sha256(&raw) == sha,
        "private report integrity fault"
    );
    Ok(raw)
}

fn unlink(dir: &Path, id: &str) -> anyhow::Result<()> {
    ensure!(wire::valid_id(id), "invalid private report id");
    match fs::remove_file(dir.join(id)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result.context("remove unpublished report"),
    }
}

#[cfg(test)]
#[path = "reports_tests.rs"]
mod tests;

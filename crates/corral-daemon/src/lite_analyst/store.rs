//! Separate, private SQLite ledger. No project, worktree, transcript or owner RPC access.
use super::{
    bootstrap::Enrollment,
    wire::{self, Binding, Fault, Fence, FenceReason, Result, Start},
};
use anyhow::{Context, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior::Immediate, params};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) struct Store {
    pub(super) db: Connection,
    pub(super) link_id: String,
    pub(super) epoch: String,
    pub(super) reconciliation_only: bool,
    // Fields drop in declaration order: keep the lock until SQLite closes.
    _lock: StateLock,
}

struct StateLock(File);

impl Drop for StateLock {
    fn drop(&mut self) {
        // Closing alone does not release flock while a forked process still holds
        // the open file description, even if its descriptor is close-on-exec.
        let _ = self.0.unlock();
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Snapshot {
    pub(super) run_id: String,
    pub(super) generation: u64,
    pub(super) seq: u64,
    pub(super) state: String,
    pub(super) reason: Option<String>,
    pub(super) offer: Option<()>, // Reports arrive with C4.
    pub(super) acked: bool,
}

pub(super) struct Started {
    pub(super) snapshot: Snapshot,
    pub(super) fresh: bool,
}

pub(super) const MAX_DEADLINE_MS: u64 = 120_000;
const MAX_RECEIPTS: u32 = 2048;
const MAX_ENROLLMENT_EPOCHS: u32 = 2048;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS links(
 link_id TEXT PRIMARY KEY, lite_instance TEXT NOT NULL, corral_instance TEXT NOT NULL,
 profile TEXT NOT NULL, manifest TEXT NOT NULL, joint_lock TEXT NOT NULL,
 epoch TEXT NOT NULL, retired INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE IF NOT EXISTS epochs(
 link_id TEXT NOT NULL REFERENCES links, epoch TEXT NOT NULL, PRIMARY KEY(link_id,epoch)
) STRICT;
CREATE TABLE IF NOT EXISTS runs(
 run_id TEXT PRIMARY KEY, link_id TEXT NOT NULL REFERENCES links,
 request_id TEXT NOT NULL, request_digest TEXT NOT NULL, grant_id TEXT NOT NULL,
 grant_generation INTEGER NOT NULL, policy_ref TEXT NOT NULL,
 generation INTEGER NOT NULL DEFAULT 1, seq INTEGER NOT NULL DEFAULT 1,
 state TEXT NOT NULL DEFAULT 'admitted', reason TEXT,
 deadline_ms INTEGER NOT NULL, admitted_ms INTEGER NOT NULL, turns INTEGER NOT NULL,
 run_cost INTEGER NOT NULL, bound_epoch TEXT, launch_phase TEXT NOT NULL DEFAULT 'intent',
 launch_id TEXT, UNIQUE(link_id,request_id)
) STRICT;
CREATE INDEX IF NOT EXISTS runs_slots ON runs(launch_phase);
";

impl Store {
    #[cfg(test)]
    pub(super) fn duplicate_lock(&self) -> std::io::Result<File> {
        self._lock.0.try_clone()
    }

    // Call only after mutual link_ready AND a common hello, before acknowledging hello.
    #[cfg(test)]
    pub(super) fn enroll(root: &Path, e: &Enrollment, now_ms: u64) -> anyhow::Result<Self> {
        Self::enroll_with(root, e, |_| Ok(((), now_ms)), || Ok(())).map(|(s, ())| s)
    }

    pub(super) fn enroll_with<T>(
        root: &Path,
        e: &Enrollment,
        load: impl FnOnce(&Connection) -> anyhow::Result<(T, u64)>,
        live: impl Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<(Self, T)> {
        live()?;
        private_directory(root)?;
        let lock = private_file(&root.join("lite-analyst.lock"))?;
        lock.try_lock().context("fixture state already owned")?;
        let lock = StateLock(lock);
        let db_path = root.join("lite-analyst.sqlite3");
        private_file(&db_path)?;
        let mut db = Connection::open(db_path).context("open fixture ledger")?;
        db.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
        db.execute_batch(SCHEMA)?;
        let (clock, earliest_ms) = load(&db)?;
        let tx = db.transaction_with_behavior(Immediate)?;
        let full: bool = tx.query_row(
            "SELECT count(*) >= ?1 FROM epochs",
            [MAX_ENROLLMENT_EPOCHS],
            |r| r.get(0),
        )?;
        ensure!(!full, "fixture enrollment receipt limit reached");
        let prior: Option<(String, bool, bool)> = tx
            .query_row(
                "SELECT epoch, retired, (lite_instance=?2 AND corral_instance=?3 AND profile=?4
                AND manifest=?5 AND joint_lock=?6) FROM links WHERE link_id=?1",
                params![
                    e.link_id,
                    e.lite_instance,
                    e.corral_instance,
                    e.profile_sha256,
                    e.corral_manifest_sha256,
                    e.joint_lock_sha256
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let reconciliation_only = match (e.prior_channel_epoch.as_deref(), prior) {
            (None, None) => {
                tx.execute("INSERT INTO links(link_id,lite_instance,corral_instance,profile,manifest,joint_lock,epoch)
                    VALUES(?1,?2,?3,?4,?5,?6,?7)", params![e.link_id,e.lite_instance,e.corral_instance,
                        e.profile_sha256,e.corral_manifest_sha256,e.joint_lock_sha256,e.channel_epoch])?;
                false
            }
            (Some(expected), Some((prior, false, true))) if expected == prior => {
                tx.execute(
                    "UPDATE links SET epoch=?2 WHERE link_id=?1",
                    params![e.link_id, e.channel_epoch],
                )?;
                // The privately provisioned epoch change is the launcher's attestation of
                // teardown of the old hosts AND descendants, not an inference from socket EOF.
                tx.execute("UPDATE runs SET state='stop_requested', seq=seq+1, reason='stopped', bound_epoch=NULL
                    WHERE link_id=?1 AND state IN ('admitted','working','waiting_for_source')", [&e.link_id])?;
                tx.execute(
                    "UPDATE runs SET launch_phase='cleaned', state='cancelled', seq=seq+1
                    WHERE link_id=?1 AND state='stop_requested'",
                    [&e.link_id],
                )?;
                true
            }
            _ => anyhow::bail!("fixture enrollment refused"),
        };
        tx.execute(
            "INSERT INTO epochs(link_id,epoch) VALUES(?1,?2)",
            params![e.link_id, e.channel_epoch],
        )
        .context("retired fixture epoch")?;
        live()?;
        tx.commit()?;
        let mut store = Self {
            db,
            link_id: e.link_id.clone(),
            epoch: e.channel_epoch.clone(),
            reconciliation_only,
            _lock: lock,
        };
        store
            .init_turns()
            .map_err(|_| anyhow::anyhow!("initialize fixture turn ledger"))?;
        store
            .finalize_turns(earliest_ms)
            .map_err(|_| anyhow::anyhow!("finalize fixture accounting"))?;
        if reconciliation_only {
            store
                .recover_turns(earliest_ms)
                .map_err(|_| anyhow::anyhow!("reconcile fixture turns"))?;
        }
        Ok((store, clock))
    }

    #[cfg(test)]
    pub(super) fn start(&mut self, request: &Start, now_ms: u64) -> Result<Started> {
        self.start_with(request, (now_ms, now_ms), || Ok(()))
    }

    pub(super) fn start_with(
        &mut self,
        request: &Start,
        (earliest_ms, now_ms): (u64, u64),
        check_live: impl FnOnce() -> anyhow::Result<()>,
    ) -> Result<Started> {
        let body = request.body()?;
        let tx = self.db.transaction_with_behavior(Immediate)?;
        require_link(&tx, &self.link_id, &self.epoch)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT run_id,request_digest FROM runs WHERE link_id=?1 AND request_id=?2",
                params![self.link_id, body.request_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, digest)) = existing {
            if digest != request.request_digest {
                return Err(Fault::Conflict);
            }
            return Ok(Started {
                snapshot: snapshot(&tx, &self.link_id, &id)?,
                fresh: false,
            });
        }
        if earliest_ms > now_ms {
            return Err(Fault::Unavailable);
        }
        if body.deadline_ms <= now_ms {
            return Err(Fault::Expired);
        }
        if self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        if body.deadline_ms - earliest_ms > MAX_DEADLINE_MS {
            return Err(Fault::Invalid);
        }
        if body.policy_ref != wire::POLICY_REF {
            return Err(Fault::PolicyUnavailable);
        }
        let l = body.limits;
        if l.calls > 8
            || l.response_bytes > 8192
            || l.total_response_bytes > 32768
            || l.turns > 4
            || !(1000..=4000).contains(&l.run_cost_microusd)
        {
            return Err(Fault::BudgetExceeded);
        }
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE launch_phase <> 'cleaned')",
            [],
            |r| r.get(0),
        )?;
        let full: bool = tx.query_row("SELECT count(*) >= ?1 FROM runs", [MAX_RECEIPTS], |r| {
            r.get(0)
        })?;
        if occupied || full {
            return Err(Fault::Busy);
        }
        let run_id = wire::fresh_id()?;
        // This row is the one durable launch intent and slot. Never persist the question,
        // vault contents, or raw approval body. Identical retries never mint a new intent.
        tx.execute("INSERT INTO runs(run_id,link_id,request_id,request_digest,grant_id,grant_generation,
            policy_ref,deadline_ms,admitted_ms,turns,run_cost) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![run_id,self.link_id,body.request_id,request.request_digest,body.grant_id,
                body.grant_generation,body.policy_ref,body.deadline_ms,now_ms,l.turns,l.run_cost_microusd])?;
        let result = snapshot(&tx, &self.link_id, &run_id)?;
        check_live()?;
        tx.commit()?;
        Ok(Started {
            snapshot: result,
            fresh: true,
        })
    }

    pub(super) fn read(&self, id: &str) -> Result<Snapshot> {
        if !wire::valid_id(id) {
            return Err(Fault::Invalid);
        }
        require_link(&self.db, &self.link_id, &self.epoch)?;
        snapshot(&self.db, &self.link_id, id)
    }

    pub(super) fn stop(
        &mut self,
        id: &str,
        generation: u64,
        reason: FenceReason,
    ) -> Result<Snapshot> {
        self.stop_run(id, generation, reason, false)
            .map(|(snapshot, _)| snapshot)
    }

    pub(super) fn stop_local(
        &mut self,
        id: &str,
        generation: u64,
        reason: FenceReason,
    ) -> Result<Option<Fence>> {
        self.stop_run(id, generation, reason, true)
            .map(|(_, notice)| notice)
    }

    fn stop_run(
        &mut self,
        id: &str,
        generation: u64,
        reason: FenceReason,
        notify_lite: bool,
    ) -> Result<(Snapshot, Option<Fence>)> {
        if !wire::valid_id(id) {
            return Err(Fault::Invalid);
        }
        let tx = self.db.transaction_with_behavior(Immediate)?;
        require_link(&tx, &self.link_id, &self.epoch)?;
        let before = snapshot(&tx, &self.link_id, id)?;
        if before.generation != generation {
            return Err(Fault::StaleGeneration);
        }
        let bound = notify_lite
            && tx.query_row(
                "SELECT bound_epoch IS ?2 FROM runs WHERE run_id=?1",
                params![id, self.epoch],
                |r| r.get::<_, bool>(0),
            )?;
        let changed = tx.execute(
            "UPDATE runs SET state='stop_requested',seq=seq+1,reason=?2,bound_epoch=NULL
            WHERE run_id=?1 AND state IN ('admitted','working','waiting_for_source')",
            params![id, reason.code()],
        )?;
        let after = snapshot(&tx, &self.link_id, id)?;
        tx.commit()?;
        Ok((
            after,
            (bound && changed != 0).then(|| Fence {
                run_id: id.to_owned(),
                generation,
                reason,
            }),
        ))
    }

    pub(super) fn maintenance(&mut self, now_ms: u64) -> Result<Option<Fence>> {
        let tx = self.db.transaction_with_behavior(Immediate)?;
        // At most one admitted/working launch slot can have a bound generation.
        let notice = tx.query_row(
            "SELECT run_id,generation FROM runs WHERE link_id=?1 AND deadline_ms<=?2 AND bound_epoch=?3
              AND state IN ('admitted','working','waiting_for_source')",
            params![self.link_id, now_ms, self.epoch],
            |r| Ok(Fence { run_id:r.get(0)?, generation:r.get(1)?, reason:FenceReason::Expired }),
        ).optional()?;
        tx.execute("UPDATE runs SET state='stop_requested',seq=seq+1,reason='expired',bound_epoch=NULL
            WHERE link_id=?1 AND deadline_ms<=?2 AND state IN ('admitted','working','waiting_for_source')",
            params![self.link_id, now_ms])?;
        clean_unsubmitted(&tx, &self.link_id)?;
        tx.commit()?;
        Ok(notice)
    }

    pub(super) fn disconnected(&mut self) -> Result<()> {
        let tx = self.db.transaction_with_behavior(Immediate)?;
        stop_link(&tx, &self.link_id)?;
        tx.commit()?;
        Ok(())
    }

    // Local supervisor operation only. The caller closes the retained channels under
    // its admission gate; a peer cannot retire or re-enroll itself over the work FD.
    #[allow(dead_code, reason = "fixture runner owns enrollment revocation")]
    pub(super) fn retire(&mut self) -> Result<()> {
        let tx = self.db.transaction_with_behavior(Immediate)?;
        require_link(&tx, &self.link_id, &self.epoch)?;
        tx.execute(
            "UPDATE links SET retired=1 WHERE link_id=?1 AND epoch=?2",
            params![self.link_id, self.epoch],
        )?;
        stop_link(&tx, &self.link_id)?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn binding(&self, id: &str) -> Result<Binding> {
        require_link(&self.db, &self.link_id, &self.epoch)?;
        self.db
            .query_row(
                "SELECT request_id,request_digest,generation,grant_id,grant_generation,policy_ref
            FROM runs WHERE run_id=?1 AND link_id=?2 AND state='admitted'",
                params![id, self.link_id],
                |r| {
                    Ok(Binding {
                        request_id: r.get(0)?,
                        request_digest: r.get(1)?,
                        run_id: id.to_owned(),
                        generation: r.get(2)?,
                        grant_id: r.get(3)?,
                        grant_generation: r.get(4)?,
                        policy_ref: r.get(5)?,
                    })
                },
            )
            .optional()?
            .ok_or(Fault::Stopped)
    }

    pub(super) fn bind(&self, id: &str, generation: u64, now_ms: u64) -> Result<()> {
        require_link(&self.db, &self.link_id, &self.epoch)?;
        if self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        let changed = self.db.execute(
            "UPDATE runs SET bound_epoch=?5 WHERE run_id=?1 AND link_id=?2
            AND generation=?3 AND deadline_ms>?4 AND state='admitted'",
            params![id, self.link_id, generation, now_ms, self.epoch],
        )?;
        if changed != 1 {
            return Err(Fault::Stopped);
        }
        Ok(())
    }

    // C3 must retain this launch identity BEFORE crossing the OS boundary, serialize
    // submission with stop, and wait for verified process/registry attachment before work.
    #[allow(dead_code, reason = "C3 owns real child submission")]
    pub(super) fn submit_launch(&self, id: &str, generation: u64, now_ms: u64) -> Result<String> {
        require_link(&self.db, &self.link_id, &self.epoch)?;
        if self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        let launch_id = wire::fresh_id()?;
        let n = self.db.execute(
            "UPDATE runs SET launch_id=?6,launch_phase='submitted' WHERE run_id=?1
            AND link_id=?2 AND generation=?3 AND deadline_ms>?4 AND state='admitted'
            AND bound_epoch=?5 AND launch_phase='intent'",
            params![id, self.link_id, generation, now_ms, self.epoch, launch_id],
        )?;
        if n != 1 {
            return Err(Fault::Stopped);
        }
        Ok(launch_id)
    }

    #[allow(dead_code, reason = "C3 owns verified child attachment")]
    pub(super) fn attach_launch(
        &self,
        id: &str,
        generation: u64,
        launch: &str,
        now_ms: u64,
    ) -> Result<()> {
        require_link(&self.db, &self.link_id, &self.epoch)?;
        if self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        let n = self.db.execute(
            "UPDATE runs SET launch_phase='attached',state='working',seq=seq+1
            WHERE run_id=?1 AND link_id=?2 AND generation=?3 AND launch_id=?4 AND deadline_ms>?5
            AND bound_epoch=?6 AND state='admitted' AND launch_phase='submitted'",
            params![id, self.link_id, generation, launch, now_ms, self.epoch],
        )?;
        if n != 1 {
            return Err(Fault::Stopped);
        }
        Ok(())
    }

    #[allow(dead_code, reason = "C3 supplies explicit verified process teardown")]
    pub(super) fn confirm_teardown(&self, id: &str, generation: u64, launch: &str) -> Result<()> {
        let n = self.db.execute("UPDATE runs SET launch_phase='cleaned',state='cancelled',seq=seq+1
            WHERE run_id=?1 AND link_id=?2 AND generation=?3 AND launch_id=?4 AND state='stop_requested'
            AND launch_phase IN ('submitted','attached')", params![id,self.link_id,generation,launch])?;
        if n != 1 {
            return Err(Fault::Stopped);
        }
        Ok(())
    }
}

fn stop_link(db: &Connection, link: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE runs SET state='stop_requested',seq=seq+1,reason='stopped',bound_epoch=NULL
        WHERE link_id=?1 AND state IN ('admitted','working','waiting_for_source')",
        [link],
    )?;
    clean_unsubmitted(db, link)
}

fn clean_unsubmitted(db: &Connection, link: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE runs SET launch_phase='cleaned',state='cancelled',seq=seq+1
        WHERE link_id=?1 AND state='stop_requested' AND launch_phase='intent'",
        [link],
    )?;
    Ok(())
}

pub(super) fn require_link(db: &Connection, link: &str, epoch: &str) -> Result<()> {
    let active: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM links WHERE link_id=?1 AND epoch=?2 AND retired=0)",
        [link, epoch],
        |r| r.get(0),
    )?;
    if active { Ok(()) } else { Err(Fault::Stopped) }
}

fn snapshot(db: &Connection, link: &str, id: &str) -> Result<Snapshot> {
    db.query_row(
        "SELECT generation,seq,state,reason FROM runs WHERE link_id=?1 AND run_id=?2",
        [link, id],
        |r| {
            Ok(Snapshot {
                run_id: id.to_owned(),
                generation: r.get(0)?,
                seq: r.get(1)?,
                state: r.get(2)?,
                reason: r.get(3)?,
                offer: None,
                acked: false,
            })
        },
    )
    .optional()?
    .ok_or(Fault::NotFound)
}

fn private_directory(root: &Path) -> anyhow::Result<()> {
    let m = fs::symlink_metadata(root).context("inspect private fixture state directory")?;
    ensure!(
        m.is_dir() && m.uid() == rustix::process::geteuid().as_raw() && m.mode() & 0o077 == 0,
        "invalid private fixture state directory"
    );
    Ok(())
}

fn private_file(path: &Path) -> anyhow::Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .context("open private fixture state file")?;
    let m = f.metadata().context("inspect private fixture state file")?;
    ensure!(
        m.is_file()
            && m.uid() == rustix::process::geteuid().as_raw()
            && m.mode() & 0o077 == 0
            && m.nlink() == 1,
        "invalid private fixture state file"
    );
    Ok(f)
}

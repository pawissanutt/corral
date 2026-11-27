//! Corral's fixture-only cost ledger. The runtime owns authenticated RPC correlation,
//! channel liveness, and the admission gate shared by fences and the actual send.

use std::time::{Duration, Instant};

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use super::clock::DAY_MS;
use super::store::{Store, require_link};
use super::wire::{Fault, MAX_SAFE, POLICY_REF, Result, digest, sha256, valid_digest, valid_id};

pub(super) const HOLD_MICROUSD: u64 = 1_000;
pub(super) const COMPLETED_MICROUSD: u64 = 760;
pub(super) const MAX_RUN_MICROUSD: u64 = 4_000;
pub(super) const MAX_DAY_MICROUSD: u64 = 5_000;
const MAX_TURNS: u64 = 4;
const TTL_MS: u64 = 5_000;
const CUTOFF_MS: u64 = 300_000;
const MAX_BODY_BYTES: usize = 262_144;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReserveKind {
    ReserveTurn,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReserveBody {
    #[serde(deserialize_with = "read_v1")]
    pub(super) v: u64,
    pub(super) kind: ReserveKind,
    #[serde(deserialize_with = "read_id")]
    pub(super) run_id: String,
    #[serde(deserialize_with = "read_positive")]
    pub(super) generation: u64,
    #[serde(deserialize_with = "read_positive")]
    pub(super) step: u64,
    #[serde(deserialize_with = "read_digest")]
    pub(super) request_digest: String,
    #[serde(deserialize_with = "read_digest")]
    pub(super) policy_ref: String,
    #[serde(deserialize_with = "read_digest")]
    pub(super) input_digest: String,
    #[serde(deserialize_with = "read_safe")]
    pub(super) max_cost_microusd: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReserveTurn {
    pub(super) body: ReserveBody,
    #[serde(deserialize_with = "read_digest")]
    pub(super) reserve_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Permit {
    #[serde(deserialize_with = "read_id")]
    pub(super) permit_id: String,
    #[serde(deserialize_with = "read_safe")]
    pub(super) expires_ms: u64,
    #[serde(deserialize_with = "read_digest")]
    pub(super) reserve_digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SettleKind {
    SettleTurn,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Completed,
    NotSent,
    Indeterminate,
}

impl Outcome {
    fn charge(self) -> u64 {
        match self {
            Self::Completed => COMPLETED_MICROUSD,
            Self::NotSent => 0,
            Self::Indeterminate => HOLD_MICROUSD,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NotSent => "not_sent",
            Self::Indeterminate => "indeterminate",
        }
    }

    fn from_label(value: &str) -> Result<Self> {
        match value {
            "completed" => Ok(Self::Completed),
            "not_sent" => Ok(Self::NotSent),
            "indeterminate" => Ok(Self::Indeterminate),
            _ => Err(Fault::Unavailable),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettleBody {
    #[serde(deserialize_with = "read_v1")]
    pub(super) v: u64,
    pub(super) kind: SettleKind,
    #[serde(deserialize_with = "read_id")]
    pub(super) run_id: String,
    #[serde(deserialize_with = "read_positive")]
    pub(super) generation: u64,
    #[serde(deserialize_with = "read_positive")]
    pub(super) step: u64,
    #[serde(deserialize_with = "read_id")]
    pub(super) permit_id: String,
    #[serde(deserialize_with = "read_digest")]
    pub(super) reserve_digest: String,
    pub(super) outcome: Outcome,
    #[serde(deserialize_with = "read_safe")]
    pub(super) charged_microusd: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettleTurn {
    pub(super) body: SettleBody,
    #[serde(deserialize_with = "read_digest")]
    pub(super) settle_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettlementReceipt {
    #[serde(deserialize_with = "read_digest")]
    pub(super) settle_digest: String,
    #[serde(deserialize_with = "read_recorded")]
    pub(super) recorded: bool,
}

fn read_recorded<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<bool, D::Error> {
    if bool::deserialize(d)? {
        Ok(true)
    } else {
        Err(D::Error::custom("expected recorded receipt"))
    }
}

fn read_v1<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    match u64::deserialize(d)? {
        1 => Ok(1),
        _ => Err(D::Error::custom("unsupported version")),
    }
}

fn read_safe<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    let n = u64::deserialize(d)?;
    if n <= MAX_SAFE {
        Ok(n)
    } else {
        Err(D::Error::custom("not a safe integer"))
    }
}

fn read_positive<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    match read_safe(d)? {
        0 => Err(D::Error::custom("expected a positive integer")),
        n => Ok(n),
    }
}

fn read_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if valid_id(&s) {
        Ok(s)
    } else {
        Err(D::Error::custom("invalid ID"))
    }
}

fn read_digest<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if valid_digest(&s) {
        Ok(s)
    } else {
        Err(D::Error::custom("invalid digest"))
    }
}

/// Original durable hold and in-process timing anchor. Never serialize the handle.
#[derive(Debug)]
pub(super) struct HeldTurn {
    request: ReserveTurn,
    link_id: String,
    epoch: String,
    launch_id: String,
    pub(super) deadline_ms: u64,
    started_ms: u64,
    started: Instant,
}

impl HeldTurn {
    /// The immutable request, also used to identify the turn for settlement.
    pub(super) fn request(&self) -> &ReserveTurn {
        &self.request
    }

    /// Request for the first reserve or an exact in-flight reserve retry only.
    /// The runtime must still match the active channel and serialize its RPCs.
    pub(super) fn reserve_request(&self, now_ms: u64, now: Instant) -> Result<&ReserveTurn> {
        if now_ms < self.started_ms || now_ms > MAX_SAFE || now < self.started {
            return Err(Fault::Unavailable);
        }
        if now_ms >= self.deadline_ms
            || now_ms - self.started_ms >= TTL_MS
            || now.duration_since(self.started) >= Duration::from_millis(TTL_MS)
        {
            return Err(Fault::Expired);
        }
        Ok(self.request())
    }
}

/// A permit verified on the original live binding; not reconstructible from disk.
#[derive(Debug)]
pub(super) struct PermitLease {
    request: ReserveTurn,
    permit: Permit,
    link_id: String,
    epoch: String,
    launch_id: String,
    observed_ms: u64,
    observed: Instant,
    deadline: Instant,
}

impl PermitLease {
    #[cfg(feature = "lite-analyst-faults")]
    pub(super) fn send_deadline(&self) -> Instant {
        self.deadline
    }

    /// Recheck immediately at I/O, after the durable consume may have waited on disk.
    /// This is a time check, not send authorization: the runtime still holds the
    /// current-channel/stop gate and must have just successfully consumed once.
    pub(super) fn check_send_time(&self, now_ms: u64, now: Instant) -> Result<()> {
        if now_ms < self.observed_ms || now_ms > MAX_SAFE || now < self.observed {
            return Err(Fault::Unavailable);
        }
        if now_ms >= self.permit.expires_ms || now >= self.deadline {
            return Err(Fault::Expired);
        }
        Ok(())
    }
}

struct ActiveRun {
    request_digest: String,
    deadline_ms: u64,
    turns: u64,
    run_cost: u64,
}

fn active_run(
    tx: &Transaction<'_>,
    link_id: &str,
    epoch: &str,
    run_id: &str,
    generation: u64,
    launch_id: &str,
    now_ms: u64,
) -> Result<ActiveRun> {
    require_link(tx, link_id, epoch)?;
    let row = tx
        .query_row(
            "SELECT generation,state,bound_epoch,launch_phase,launch_id,
                    request_digest,deadline_ms,turns,run_cost,policy_ref
             FROM runs WHERE link_id=?1 AND run_id=?2",
            params![link_id, run_id],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    ActiveRun {
                        request_digest: row.get(5)?,
                        deadline_ms: row.get(6)?,
                        turns: row.get(7)?,
                        run_cost: row.get(8)?,
                    },
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?
        .ok_or(Fault::NotFound)?;
    let (current, state, bound_epoch, phase, attached_id, run, policy_ref) = row;
    if current != generation {
        return Err(Fault::StaleGeneration);
    }
    if now_ms >= run.deadline_ms {
        return Err(Fault::Expired);
    }
    if state != "working"
        || phase != "attached"
        || attached_id.as_deref() != Some(launch_id)
        || bound_epoch.as_deref() != Some(epoch)
    {
        return Err(Fault::Stopped);
    }
    if policy_ref != POLICY_REF {
        return Err(Fault::PolicyUnavailable);
    }
    if !valid_digest(&run.request_digest) || run.deadline_ms > MAX_SAFE - CUTOFF_MS {
        return Err(Fault::Unavailable);
    }
    Ok(run)
}

impl Store {
    pub(super) fn init_turns(&self) -> Result<()> {
        self.db.execute_batch(
            "CREATE TABLE IF NOT EXISTS turn_ledger (
                link_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                step INTEGER NOT NULL,
                request_digest TEXT NOT NULL,
                input_digest TEXT NOT NULL,
                reserve_digest TEXT NOT NULL,
                reserved_day INTEGER NOT NULL,
                cutoff_ms INTEGER NOT NULL,
                permit_id TEXT,
                permit_expires_ms INTEGER,
                consumed INTEGER NOT NULL DEFAULT 0,
                outcome TEXT,
                settle_digest TEXT,
                receipt_observed INTEGER NOT NULL DEFAULT 0,
                cutoff_defaulted INTEGER NOT NULL DEFAULT 0,
                budget_charge INTEGER,
                PRIMARY KEY (link_id, run_id, step),
                UNIQUE (link_id, permit_id),
                CHECK ((permit_id IS NULL) = (permit_expires_ms IS NULL)),
                CHECK ((outcome IS NULL) = (settle_digest IS NULL)),
                CHECK (consumed IN (0, 1) AND receipt_observed IN (0, 1) AND cutoff_defaulted IN (0, 1))
            ) STRICT;
            CREATE INDEX IF NOT EXISTS turn_ledger_day ON turn_ledger(reserved_day);
            CREATE INDEX IF NOT EXISTS turn_ledger_cutoff ON turn_ledger(cutoff_ms)
                WHERE budget_charge IS NULL;",
        )?;
        Ok(())
    }

    /// Hold the fixture maximum and debit a turn before any reverse RPC. The
    /// body is already finalized and registry/route-checked by the trusted host.
    pub(super) fn hold_turn_with(
        &mut self,
        run_id: &str,
        generation: u64,
        launch_id: &str,
        finalized_body: &[u8],
        (now_ms, now): (u64, Instant),
        check_time: impl FnOnce() -> anyhow::Result<u64>,
    ) -> Result<HeldTurn> {
        if !valid_id(run_id) || !valid_id(launch_id) || !(1..=MAX_SAFE).contains(&generation) {
            return Err(Fault::Invalid);
        }
        let input_digest = provider_digest(finalized_body)?;
        if self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = active_run(
            &tx,
            &self.link_id,
            &self.epoch,
            run_id,
            generation,
            launch_id,
            now_ms,
        )?;
        let previous = tx
            .query_row(
                "SELECT step, outcome, receipt_observed, cutoff_defaulted FROM turn_ledger
                 WHERE link_id=?1 AND run_id=?2 ORDER BY step DESC LIMIT 1",
                params![&self.link_id, run_id],
                |r| {
                    Ok((
                        r.get::<_, u64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()?;
        let step = match previous {
            None => 1,
            Some((_, _, _, true)) => return Err(Fault::Stopped),
            Some((_, _, false, false)) => return Err(Fault::Busy),
            Some((step, Some(outcome), true, false)) => match Outcome::from_label(&outcome)? {
                Outcome::Indeterminate => return Err(Fault::Stopped),
                _ => step + 1,
            },
            Some(_) => return Err(Fault::Unavailable),
        };
        let day = now_ms / DAY_MS;
        let run_spend: u64 = tx.query_row(
            "SELECT COALESCE(SUM(COALESCE(budget_charge,?3)),0) FROM turn_ledger
             WHERE link_id=?1 AND run_id=?2",
            params![&self.link_id, run_id, HOLD_MICROUSD],
            |r| r.get(0),
        )?;
        // The fixed owner ceiling covers this whole store, so re-enrolling
        // under another link cannot reset spend or strand its pending holds.
        let day_spend: u64 = tx.query_row(
            "SELECT COALESCE(SUM(COALESCE(budget_charge,?2)),0) FROM turn_ledger
             WHERE reserved_day=?1 OR budget_charge IS NULL",
            params![day, HOLD_MICROUSD],
            |r| r.get(0),
        )?;
        if step > run.turns.min(MAX_TURNS)
            || run_spend.saturating_add(HOLD_MICROUSD) > run.run_cost.min(MAX_RUN_MICROUSD)
            || day_spend.saturating_add(HOLD_MICROUSD) > MAX_DAY_MICROUSD
        {
            return Err(Fault::BudgetExceeded);
        }
        let body = ReserveBody {
            v: 1,
            kind: ReserveKind::ReserveTurn,
            run_id: run_id.to_owned(),
            generation,
            step,
            request_digest: run.request_digest,
            policy_ref: POLICY_REF.to_owned(),
            input_digest,
            max_cost_microusd: HOLD_MICROUSD,
        };
        let reserve_digest = digest(&body)?;
        tx.execute(
            "INSERT INTO turn_ledger(link_id,run_id,generation,step,request_digest,input_digest,
                                     reserve_digest,reserved_day,cutoff_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                &self.link_id,
                run_id,
                generation,
                step,
                &body.request_digest,
                &body.input_digest,
                &reserve_digest,
                day,
                run.deadline_ms + CUTOFF_MS
            ],
        )?;
        let checked_ms = check_time()?;
        if checked_ms / DAY_MS != day || checked_ms >= run.deadline_ms {
            return Err(Fault::Expired);
        }
        tx.commit()?;
        Ok(HeldTurn {
            request: ReserveTurn {
                body,
                reserve_digest,
            },
            link_id: self.link_id.clone(),
            epoch: self.epoch.clone(),
            launch_id: launch_id.to_owned(),
            deadline_ms: run.deadline_ms,
            started_ms: now_ms,
            started: now,
        })
    }

    /// Called only for the correlated reserve response from the authenticated
    /// current peer. Issue one local lease, never extending its original clock.
    pub(super) fn accept_permit(
        &mut self,
        held: &HeldTurn,
        permit: Permit,
        now_ms: u64,
        now: Instant,
    ) -> Result<PermitLease> {
        if self.reconciliation_only || held.epoch != self.epoch || held.link_id != self.link_id {
            return Err(Fault::Stopped);
        }
        let request = held.reserve_request(now_ms, now)?;
        if !valid_id(&permit.permit_id) || !valid_digest(&permit.reserve_digest) {
            return Err(Fault::Invalid);
        }
        if permit.reserve_digest != request.reserve_digest {
            return Err(Fault::Conflict);
        }
        if permit.expires_ms <= now_ms {
            return Err(Fault::Expired);
        }
        if permit.expires_ms > held.deadline_ms || permit.expires_ms - now_ms > TTL_MS {
            return Err(Fault::Invalid);
        }
        // No RTT or clock re-sampling can extend the original monotonic window.
        // Allow for truncation when the runtime samples epoch milliseconds.
        let remaining = (permit.expires_ms - held.started_ms)
            .min(TTL_MS)
            .saturating_sub(1);
        let deadline = held
            .started
            .checked_add(Duration::from_millis(remaining))
            .ok_or(Fault::Unavailable)?;
        if now >= deadline {
            return Err(Fault::Expired);
        }
        let body = &request.body;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = active_run(
            &tx,
            &self.link_id,
            &self.epoch,
            &body.run_id,
            body.generation,
            &held.launch_id,
            now_ms,
        )?;
        if run.deadline_ms != held.deadline_ms || run.request_digest != body.request_digest {
            return Err(Fault::Conflict);
        }
        let changed = tx.execute(
            "UPDATE turn_ledger SET permit_id=?1, permit_expires_ms=?2
             WHERE link_id=?3 AND run_id=?4 AND generation=?5 AND step=?6 AND reserve_digest=?7
               AND permit_id IS NULL AND consumed=0 AND outcome IS NULL AND budget_charge IS NULL
               AND NOT EXISTS(SELECT 1 FROM turn_ledger WHERE link_id=?3 AND permit_id=?1)",
            params![
                &permit.permit_id,
                permit.expires_ms,
                &self.link_id,
                &body.run_id,
                body.generation,
                body.step,
                &request.reserve_digest
            ],
        )?;
        if changed != 1 {
            return Err(Fault::Conflict);
        }
        tx.commit()?;
        Ok(PermitLease {
            request: request.clone(),
            permit,
            link_id: self.link_id.clone(),
            epoch: self.epoch.clone(),
            launch_id: held.launch_id.clone(),
            observed_ms: now_ms,
            observed: now,
            deadline,
        })
    }

    /// The one-way paid-send CAS. Caller must hold the live admission gate
    /// across consume and the one transport send. Failure never authorizes I/O.
    pub(super) fn consume_turn(
        &mut self,
        lease: &PermitLease,
        finalized_body: &[u8],
        now_ms: u64,
        now: Instant,
    ) -> Result<()> {
        if self.reconciliation_only || lease.epoch != self.epoch || lease.link_id != self.link_id {
            return Err(Fault::Stopped);
        }
        lease.check_send_time(now_ms, now)?;
        let input_digest = provider_digest(finalized_body)?;
        let request = &lease.request;
        let body = &request.body;
        if input_digest != body.input_digest {
            return Err(Fault::Conflict);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = active_run(
            &tx,
            &self.link_id,
            &self.epoch,
            &body.run_id,
            body.generation,
            &lease.launch_id,
            now_ms,
        )?;
        if run.request_digest != body.request_digest || run.deadline_ms < lease.permit.expires_ms {
            return Err(Fault::Conflict);
        }
        let changed = tx.execute(
            "UPDATE turn_ledger SET consumed=1
             WHERE link_id=?1 AND run_id=?2 AND generation=?3 AND step=?4 AND reserve_digest=?5
               AND permit_id=?6 AND permit_expires_ms=?7 AND input_digest=?8 AND consumed=0
               AND outcome IS NULL AND budget_charge IS NULL",
            params![
                &self.link_id,
                &body.run_id,
                body.generation,
                body.step,
                &request.reserve_digest,
                &lease.permit.permit_id,
                lease.permit.expires_ms,
                &input_digest
            ],
        )?;
        if changed != 1 {
            return Err(Fault::Conflict);
        }
        tx.commit()?;
        Ok(())
    }

    /// Seal a trusted fixture outcome before the settlement RPC. In particular,
    /// a consumed turn can never be marked not sent, even after process loss.
    pub(super) fn seal_turn(
        &mut self,
        request: &ReserveTurn,
        outcome: Outcome,
        now_ms: u64,
    ) -> Result<SettleTurn> {
        self.finalize_turns(now_ms)?;
        let body = &request.body;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = load_turn(&tx, &self.link_id, &body.run_id, body.generation, body.step)?;
        if row.reserve != *request {
            return Err(Fault::Conflict);
        }
        let sent = row.seal(&tx, &self.link_id, outcome)?;
        tx.commit()?;
        Ok(sent)
    }

    /// Bounded backlog; the runtime sends one at a time, only to the same
    /// authenticated enrollment. A sealed row is never a send authorization.
    pub(super) fn pending_settlements(&mut self, now_ms: u64) -> Result<Vec<SettleTurn>> {
        self.finalize_turns(now_ms)?;
        require_link(&self.db, &self.link_id, &self.epoch)?;
        let mut q = self.db.prepare(&format!(
            "{TURN_SELECT} WHERE link_id=?1 AND outcome IS NOT NULL AND receipt_observed=0
             ORDER BY cutoff_ms, run_id, step LIMIT 16"
        ))?;
        let rows = q.query_map([&self.link_id], TurnRecord::read)?;
        rows.map(|r| r?.sealed()).collect()
    }

    /// Only a correlated, authenticated receipt for the exact sealed payload
    /// resolves the step. No credit is possible once the cutoff is finalized.
    pub(super) fn observe_settlement_at(
        &mut self,
        sent: &SettleTurn,
        receipt: &SettlementReceipt,
        earliest_ms: u64,
        latest_ms: u64,
    ) -> Result<()> {
        if earliest_ms > latest_ms || latest_ms > MAX_SAFE {
            return Err(Fault::Invalid);
        }
        self.finalize_turns(earliest_ms)?;
        let body = &sent.body;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_link(&tx, &self.link_id, &self.epoch)?;
        let row = load_turn(&tx, &self.link_id, &body.run_id, body.generation, body.step)?;
        if !receipt.recorded
            || receipt.settle_digest != sent.settle_digest
            || row.sealed()? != *sent
        {
            return Err(Fault::Conflict);
        }
        let changed = tx.execute(
            "UPDATE turn_ledger SET receipt_observed=1,
             budget_charge=CASE WHEN receipt_observed=0 AND cutoff_ms>?7
                 THEN COALESCE(budget_charge,?1) ELSE budget_charge END
             WHERE link_id=?2 AND run_id=?3 AND generation=?4 AND step=?5 AND settle_digest=?6",
            params![
                body.charged_microusd,
                &self.link_id,
                &body.run_id,
                body.generation,
                body.step,
                &sent.settle_digest,
                latest_ms,
            ],
        )?;
        if changed != 1 {
            return Err(Fault::Conflict);
        }
        tx.commit()?;
        Ok(())
    }

    /// Permanently charge unresolved allowance (including lost reserve replies)
    /// to the UTC day of the original hold, at or after the pinned horizon.
    pub(super) fn finalize_turns(&mut self, now_ms: u64) -> Result<usize> {
        if now_ms > MAX_SAFE {
            return Err(Fault::Unavailable);
        }
        Ok(self.db.execute(
            "UPDATE turn_ledger SET budget_charge=?1, cutoff_defaulted=1
             WHERE budget_charge IS NULL AND cutoff_ms<=?2",
            params![HOLD_MICROUSD, now_ms],
        )?)
    }

    /// Re-exec with proved teardown can settle a known permit but may never
    /// recover an authorizing lease or infer completion of a consumed turn.
    pub(super) fn recover_turns(&mut self, now_ms: u64) -> Result<()> {
        if !self.reconciliation_only {
            return Err(Fault::Stopped);
        }
        self.finalize_turns(now_ms)?;
        loop {
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let records = {
                let mut q = tx.prepare(&format!(
                    "{TURN_SELECT} WHERE link_id=?1 AND permit_id IS NOT NULL AND outcome IS NULL
                     ORDER BY cutoff_ms, run_id, step LIMIT 16"
                ))?;
                q.query_map([&self.link_id], TurnRecord::read)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            if records.is_empty() {
                return Ok(());
            }
            for row in records {
                let outcome = if row.consumed {
                    Outcome::Indeterminate
                } else {
                    Outcome::NotSent
                };
                row.seal(&tx, &self.link_id, outcome)?;
            }
            tx.commit()?;
        }
    }
}

fn provider_digest(finalized_body: &[u8]) -> Result<String> {
    if finalized_body.is_empty() || finalized_body.len() > MAX_BODY_BYTES {
        return Err(Fault::Invalid);
    }
    Ok(sha256(finalized_body))
}

const TURN_SELECT: &str =
    "SELECT run_id,generation,step,request_digest,input_digest,reserve_digest,
                                  permit_id,consumed,outcome,settle_digest FROM turn_ledger";

struct TurnRecord {
    reserve: ReserveTurn,
    permit_id: Option<String>,
    consumed: bool,
    outcome: Option<String>,
    settle_digest: Option<String>,
}

fn load_turn(
    tx: &Transaction<'_>,
    link_id: &str,
    run_id: &str,
    generation: u64,
    step: u64,
) -> Result<TurnRecord> {
    tx.query_row(
        &format!("{TURN_SELECT} WHERE link_id=?1 AND run_id=?2 AND generation=?3 AND step=?4"),
        params![link_id, run_id, generation, step],
        TurnRecord::read,
    )
    .optional()?
    .ok_or(Fault::NotFound)
}

impl TurnRecord {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            reserve: ReserveTurn {
                body: ReserveBody {
                    v: 1,
                    kind: ReserveKind::ReserveTurn,
                    run_id: row.get(0)?,
                    generation: row.get(1)?,
                    step: row.get(2)?,
                    request_digest: row.get(3)?,
                    policy_ref: POLICY_REF.to_owned(),
                    input_digest: row.get(4)?,
                    max_cost_microusd: HOLD_MICROUSD,
                },
                reserve_digest: row.get(5)?,
            },
            permit_id: row.get(6)?,
            consumed: row.get(7)?,
            outcome: row.get(8)?,
            settle_digest: row.get(9)?,
        })
    }

    fn settlement(&self, outcome: Outcome) -> Result<SettleTurn> {
        let reserve = &self.reserve.body;
        let body = SettleBody {
            v: 1,
            kind: SettleKind::SettleTurn,
            run_id: reserve.run_id.clone(),
            generation: reserve.generation,
            step: reserve.step,
            permit_id: self.permit_id.clone().ok_or(Fault::Unavailable)?,
            reserve_digest: self.reserve.reserve_digest.clone(),
            outcome,
            charged_microusd: outcome.charge(),
        };
        let settle_digest = digest(&body)?;
        Ok(SettleTurn {
            body,
            settle_digest,
        })
    }

    fn sealed(&self) -> Result<SettleTurn> {
        let outcome = Outcome::from_label(self.outcome.as_deref().ok_or(Fault::Conflict)?)?;
        let sent = self.settlement(outcome)?;
        if self.settle_digest.as_deref() != Some(sent.settle_digest.as_str()) {
            return Err(Fault::Unavailable);
        }
        Ok(sent)
    }

    fn seal(&self, tx: &Transaction<'_>, link_id: &str, outcome: Outcome) -> Result<SettleTurn> {
        if let Some(existing) = &self.outcome {
            return if existing == outcome.label() {
                self.sealed()
            } else {
                Err(Fault::Conflict)
            };
        }
        if self.consumed == (outcome == Outcome::NotSent) {
            return Err(Fault::Conflict);
        }
        let sent = self.settlement(outcome)?;
        let key = &sent.body;
        let changed = tx.execute(
            "UPDATE turn_ledger SET outcome=?1, settle_digest=?2
             WHERE link_id=?3 AND run_id=?4 AND generation=?5 AND step=?6 AND outcome IS NULL",
            params![
                outcome.label(),
                &sent.settle_digest,
                link_id,
                &key.run_id,
                key.generation,
                key.step
            ],
        )?;
        if changed != 1 {
            return Err(Fault::Conflict);
        }
        Ok(sent)
    }
}

#[cfg(test)]
#[path = "turns_tests.rs"]
mod tests;

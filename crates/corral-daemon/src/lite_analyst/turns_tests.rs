use super::super::{bootstrap::Enrollment, wire};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use tempfile::TempDir;

const BODY: &[u8] = br#"{"model":"fixture-v1","stream":true}"#;
const REQUEST_DIGEST: &str = "3e4dd4a7ea898abac0731096c6f2104c803c78b162d68501fccaf4ccc366fa98";
const AT: u64 = 120_000;
const DEADLINE: u64 = AT + 30_000;
const LAUNCH: &str = "launch_A";

fn enrollment(link: &str, epoch: &str, prior: Option<&str>) -> Enrollment {
    Enrollment {
        link_id: link.into(),
        lite_instance: "lite_A".into(),
        corral_instance: "corral_A".into(),
        channel_epoch: epoch.into(),
        prior_channel_epoch: prior.map(str::to_owned),
        profile_sha256: "1".repeat(64),
        corral_manifest_sha256: "2".repeat(64),
        joint_lock_sha256: "3".repeat(64),
    }
}

fn create() -> (TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let store = open(root.path(), "link_A", "epoch_A", None, AT);
    (root, store)
}

fn open(root: &Path, link: &str, epoch: &str, prior: Option<&str>, now: u64) -> Store {
    Store::enroll(root, &enrollment(link, epoch, prior), now).unwrap()
}

// Arrange precisely the row C2 consumes, without exercising lifecycle launch or RPC.
fn attach(store: &Store, id: &str, deadline: u64, turns: u64, run_cost: u64) {
    store.db.execute(
        "INSERT INTO runs(run_id, link_id, request_id, request_digest, grant_id, grant_generation,
                          policy_ref, state, deadline_ms, admitted_ms, turns, run_cost,
                          bound_epoch, launch_phase, launch_id)
         VALUES(?1, ?2, ?1, ?3, 'grant', 1, ?4, 'working', ?5, ?5-30000, ?6, ?7,
                ?8, 'attached', ?9)",
        params![id, &store.link_id, REQUEST_DIGEST, POLICY_REF, deadline, turns, run_cost,
                &store.epoch, LAUNCH],
    ).unwrap();
}

fn close_run(store: &Store, id: &str) {
    store
        .db
        .execute(
            "UPDATE runs SET state='failed', bound_epoch=NULL, launch_phase='cleaned'
                      WHERE run_id=?1",
            [id],
        )
        .unwrap();
}

fn permit(held: &HeldTurn, permit_id: &str, expires_ms: u64) -> Permit {
    Permit {
        permit_id: permit_id.into(),
        expires_ms,
        reserve_digest: held.request().reserve_digest.clone(),
    }
}

impl Store {
    fn hold_turn(
        &mut self,
        run: &str,
        generation: u64,
        launch: &str,
        body: &[u8],
        wall: u64,
        mono: Instant,
    ) -> Result<HeldTurn> {
        self.hold_turn_with(run, generation, launch, body, (wall, mono), || Ok(wall))
    }

    fn observe_settlement(
        &mut self,
        sent: &SettleTurn,
        receipt: &SettlementReceipt,
        now: u64,
    ) -> Result<()> {
        self.observe_settlement_at(sent, receipt, now, now)
    }
}

fn receipt(sent: &SettleTurn) -> SettlementReceipt {
    SettlementReceipt {
        settle_digest: sent.settle_digest.clone(),
        recorded: true,
    }
}

fn budget(store: &Store, run: &str) -> (bool, Option<u64>, bool) {
    store.db.query_row(
        "SELECT consumed,budget_charge,receipt_observed FROM turn_ledger WHERE link_id=?1 AND run_id=?2 AND step=1",
        params![&store.link_id, run],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap()
}

#[test]
fn turn_dtos_and_preimages_match_pinned_t2069_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../../docs/contracts/lite-analyst-v1/vectors.json"
    ))
    .unwrap();
    let reserve = &vectors["reserve"]["signature"];
    let permit: Permit = serde_json::from_value(vectors["reserve"]["result"].clone()).unwrap();
    let outbound = STANDARD
        .decode(vectors["model_request_b64"].as_str().unwrap())
        .unwrap();
    let (_root, mut store) = create();
    let now = permit.expires_ms - TTL_MS;
    let mono = Instant::now();
    attach(&store, "run_A", permit.expires_ms + 1, 4, MAX_RUN_MICROUSD);
    let held = store
        .hold_turn("run_A", 1, LAUNCH, &outbound, now, mono)
        .unwrap();
    assert_eq!(
        held.request().body,
        serde_json::from_value::<ReserveBody>(reserve["body"].clone()).unwrap()
    );
    assert_eq!(held.request().reserve_digest, reserve["sha256"]);
    assert_eq!(
        wire::canonical(&held.request().body).unwrap(),
        reserve["canonical_utf8"].as_str().unwrap().as_bytes()
    );
    let lease = store.accept_permit(&held, permit, now, mono).unwrap();
    store.consume_turn(&lease, &outbound, now, mono).unwrap();
    let sent = store
        .seal_turn(held.request(), Outcome::Completed, now)
        .unwrap();
    let settlement = &vectors["settlement"]["completed"];
    assert_eq!(
        serde_json::to_value(&sent.body).unwrap(),
        settlement["body"]
    );
    assert_eq!(sent.settle_digest, settlement["sha256"]);
    for outcome in ["completed", "indeterminate", "not_sent"] {
        let vector = &vectors["settlement"][outcome];
        let decoded: SettleBody = serde_json::from_value(vector["body"].clone()).unwrap();
        assert_eq!(digest(&decoded).unwrap(), vector["sha256"]);
        assert_eq!(
            wire::canonical(&decoded).unwrap(),
            vector["canonical_utf8"].as_str().unwrap().as_bytes()
        );
    }
}

#[test]
fn peer_turn_replies_are_closed_and_strict() {
    let base = format!(r#"{{"permit_id":"p","expires_ms":1,"reserve_digest":"{REQUEST_DIGEST}"}}"#);
    assert!(wire::decode::<Permit>(base.as_bytes()).is_ok());
    for changed in [
        base.replace("1,", "1.0,"),
        base.replace("1,", "9007199254740992,"),
        base.replace("1,", "null,"),
        base.replace("1,", "1,\"expires_ms\":2,"),
        base.replace("1,", "1,\"extra\":true,"),
        base.replace("\"expires_ms\":1,", ""),
        base.replace("\"p\"", "\"p/a\""),
        base.replace(REQUEST_DIGEST, &REQUEST_DIGEST.to_ascii_uppercase()),
    ] {
        assert!(wire::decode::<Permit>(changed.as_bytes()).is_err());
    }
    for result in [
        format!(r#"{{"settle_digest":"{REQUEST_DIGEST}","recorded":"true"}}"#),
        format!(r#"{{"settle_digest":"{REQUEST_DIGEST}","recorded":false}}"#),
        format!(r#"{{"settle_digest":"{REQUEST_DIGEST}","recorded":true,"recorded":true}}"#),
        format!(r#"{{"settle_digest":"{REQUEST_DIGEST}"}}"#),
    ] {
        assert!(wire::decode::<SettlementReceipt>(result.as_bytes()).is_err());
    }
}

#[test]
fn holds_and_exact_consume_gate_each_resolved_step() {
    let (_root, mut store) = create();
    attach(&store, "run_A", DEADLINE, 2, 1_760);
    let mono = Instant::now();
    let held = store.hold_turn("run_A", 1, LAUNCH, BODY, AT, mono).unwrap();
    assert_eq!(held.request().body.step, 1);
    assert_eq!(budget(&store, "run_A"), (false, None, false));
    assert_eq!(
        held.reserve_request(AT + 100, mono).unwrap(),
        held.request()
    );
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::Busy
    );
    let mut wrong = permit(&held, "permit_A", AT + 1_000);
    wrong.reserve_digest = "0".repeat(64);
    assert_eq!(
        store.accept_permit(&held, wrong, AT, mono).unwrap_err(),
        Fault::Conflict
    );
    let make_permit = || permit(&held, "permit_A", AT + 1_000);
    let lease = store.accept_permit(&held, make_permit(), AT, mono).unwrap();
    assert_eq!(
        store
            .accept_permit(&held, make_permit(), AT, mono)
            .unwrap_err(),
        Fault::Conflict
    );
    assert_eq!(
        store.consume_turn(&lease, b"different bytes", AT, mono),
        Err(Fault::Conflict)
    );
    store.consume_turn(&lease, BODY, AT, mono).unwrap();
    assert_eq!(
        store.consume_turn(&lease, BODY, AT, mono),
        Err(Fault::Conflict)
    );
    assert_eq!(
        store.seal_turn(held.request(), Outcome::NotSent, AT),
        Err(Fault::Conflict)
    );
    let sent = store
        .seal_turn(held.request(), Outcome::Completed, AT)
        .unwrap();
    assert_eq!(
        store
            .seal_turn(held.request(), Outcome::Completed, AT)
            .unwrap(),
        sent
    );
    assert_eq!(sent.body.charged_microusd, 760);
    assert_eq!(
        store.pending_settlements(AT).unwrap().as_slice(),
        std::slice::from_ref(&sent)
    );
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::Busy
    );
    let mut other = receipt(&sent);
    other.recorded = false;
    assert_eq!(
        store.observe_settlement(&sent, &other, AT),
        Err(Fault::Conflict)
    );
    other.recorded = true;
    other.settle_digest = "f".repeat(64);
    assert_eq!(
        store.observe_settlement(&sent, &other, AT),
        Err(Fault::Conflict)
    );
    store
        .observe_settlement(&sent, &receipt(&sent), AT)
        .unwrap();
    store
        .observe_settlement(&sent, &receipt(&sent), AT)
        .unwrap();
    assert!(store.pending_settlements(AT).unwrap().is_empty());
    assert_eq!(budget(&store, "run_A"), (true, Some(760), true));
    let next = store.hold_turn("run_A", 1, LAUNCH, BODY, AT, mono).unwrap();
    assert_eq!(next.request().body.step, 2);
    store
        .accept_permit(&next, permit(&next, "permit_B", AT + 1_000), AT, mono)
        .unwrap();
    let sent = store
        .seal_turn(next.request(), Outcome::NotSent, AT)
        .unwrap();
    store
        .observe_settlement(&sent, &receipt(&sent), AT)
        .unwrap();
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::BudgetExceeded
    );
}

#[test]
fn permit_lease_uses_original_monotonic_window_and_live_launch_fence() {
    let (_root, mut store) = create();
    attach(&store, "run_A", DEADLINE, 4, MAX_RUN_MICROUSD);
    let mono = Instant::now();
    assert_eq!(
        store
            .hold_turn("run_A", 1, "wrong_launch", BODY, AT, mono)
            .unwrap_err(),
        Fault::Stopped
    );
    assert_eq!(
        store
            .hold_turn("run_A", 2, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::StaleGeneration
    );
    let held = store.hold_turn("run_A", 1, LAUNCH, BODY, AT, mono).unwrap();
    assert_eq!(
        held.reserve_request(AT, mono + Duration::from_millis(TTL_MS)),
        Err(Fault::Expired)
    );
    let received = mono + Duration::from_millis(4_000);
    assert_eq!(
        store
            .accept_permit(
                &held,
                permit(&held, "permit_A", AT + 9_001),
                AT + 4_000,
                received
            )
            .unwrap_err(),
        Fault::Invalid
    );
    let lease = store
        .accept_permit(
            &held,
            permit(&held, "permit_A", AT + 9_000),
            AT + 4_000,
            received,
        )
        .unwrap();
    let elapsed = mono + Duration::from_millis(TTL_MS);
    assert_eq!(
        store.consume_turn(&lease, BODY, AT + 4_999, elapsed),
        Err(Fault::Expired)
    );
    assert_eq!(
        store.consume_turn(&lease, BODY, AT + 3_999, received),
        Err(Fault::Unavailable)
    );
    store.disconnected().unwrap();
    assert_eq!(
        store.consume_turn(&lease, BODY, AT + 4_999, received),
        Err(Fault::Stopped)
    );
    let sent = store
        .seal_turn(held.request(), Outcome::NotSent, AT + 5_000)
        .unwrap();
    assert_eq!(sent.body.charged_microusd, 0);
}

#[test]
fn ambiguous_dispatch_does_not_authorize_another_step() {
    let (_root, mut store) = create();
    attach(&store, "run_A", DEADLINE, 4, MAX_RUN_MICROUSD);
    let mono = Instant::now();
    let held = store.hold_turn("run_A", 1, LAUNCH, BODY, AT, mono).unwrap();
    let lease = store
        .accept_permit(&held, permit(&held, "permit_A", AT + 1_000), AT, mono)
        .unwrap();
    store.consume_turn(&lease, BODY, AT, mono).unwrap();
    let sent = store
        .seal_turn(held.request(), Outcome::Indeterminate, AT)
        .unwrap();
    assert_eq!(sent.body.charged_microusd, HOLD_MICROUSD);
    store
        .observe_settlement(&sent, &receipt(&sent), AT)
        .unwrap();
    assert_eq!(
        store.seal_turn(held.request(), Outcome::Completed, AT),
        Err(Fault::Conflict)
    );
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::Stopped
    );
}

#[test]
fn rollover_during_reservation_does_not_commit_to_previous_day() {
    let (_root, mut store) = create();
    let mono = Instant::now();
    attach(&store, "run_A", DAY_MS + 30_000, 4, MAX_RUN_MICROUSD);
    assert_eq!(
        store
            .hold_turn_with("run_A", 1, LAUNCH, BODY, (DAY_MS - 1, mono), || Ok(DAY_MS))
            .unwrap_err(),
        Fault::Expired
    );
    let valid = store
        .hold_turn("run_A", 1, LAUNCH, BODY, DAY_MS, mono)
        .unwrap();
    assert_eq!(valid.request().body.step, 1);
}

#[test]
fn unresolved_holds_carry_over_utc_midnight_and_cutoff_cannot_credit() {
    let (_root, mut store) = create();
    let mono = Instant::now();
    let yesterday = DAY_MS - 100;
    let horizon = DAY_MS + 30_000 + CUTOFF_MS;
    let mut known = None;
    for i in 0..5 {
        let id = format!("old_{i}");
        attach(&store, &id, DAY_MS + 30_000, 4, MAX_RUN_MICROUSD);
        let held = store
            .hold_turn(&id, 1, LAUNCH, BODY, yesterday, mono)
            .unwrap();
        if i == 0 {
            store
                .accept_permit(
                    &held,
                    permit(&held, "known_permit", yesterday + TTL_MS),
                    yesterday,
                    mono,
                )
                .unwrap();
            known = Some(held);
        }
        close_run(&store, &id);
    }
    attach(&store, "today", horizon + 1_000, 4, MAX_RUN_MICROUSD);
    assert_eq!(
        store
            .hold_turn("today", 1, LAUNCH, BODY, DAY_MS, mono)
            .unwrap_err(),
        Fault::BudgetExceeded
    );
    let late = store
        .seal_turn(known.unwrap().request(), Outcome::NotSent, horizon - 1)
        .unwrap();
    // Clock uncertainty overlaps cutoff: accept the receipt but keep the full
    // unresolved hold carried into this UTC day until definitely past cutoff.
    store
        .observe_settlement_at(&late, &receipt(&late), horizon - 1, horizon)
        .unwrap();
    assert_eq!(budget(&store, "old_0"), (false, None, true));
    assert_eq!(
        store
            .hold_turn("today", 1, LAUNCH, BODY, horizon, mono)
            .unwrap_err(),
        Fault::BudgetExceeded
    );
    assert_eq!(store.finalize_turns(horizon).unwrap(), 5);
    assert_eq!(store.finalize_turns(horizon).unwrap(), 0);
    assert_eq!(budget(&store, "old_1"), (false, Some(1_000), false));
    assert_eq!(budget(&store, "old_0"), (false, Some(1_000), true));
    assert_eq!(
        store
            .hold_turn("today", 1, LAUNCH, BODY, horizon, mono)
            .unwrap()
            .request()
            .body
            .step,
        1
    );
}

#[test]
fn finalized_cutoff_cannot_reopen_paid_work() {
    let (_root, mut store) = create();
    let mono = Instant::now();
    attach(&store, "run_A", DEADLINE, 4, MAX_RUN_MICROUSD);
    let held = store.hold_turn("run_A", 1, LAUNCH, BODY, AT, mono).unwrap();
    store
        .accept_permit(&held, permit(&held, "permit_A", AT + 1_000), AT, mono)
        .unwrap();
    // This triggers cutoff even if maintenance hasn't updated the run's lifecycle row.
    let horizon = DEADLINE + CUTOFF_MS;
    let late = store
        .seal_turn(held.request(), Outcome::NotSent, horizon)
        .unwrap();
    store
        .observe_settlement(&late, &receipt(&late), horizon)
        .unwrap();
    assert_eq!(budget(&store, "run_A"), (false, Some(HOLD_MICROUSD), true));
    // A clock step back cannot erase the durable cutoff fence.
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::Stopped
    );
}

#[test]
fn fresh_enrollment_does_not_reset_store_day_budget() {
    let (root, mut store) = create();
    let mono = Instant::now();
    for i in 0..5 {
        let id = format!("run_{i}");
        attach(&store, &id, DEADLINE, 4, MAX_RUN_MICROUSD);
        store.hold_turn(&id, 1, LAUNCH, BODY, AT, mono).unwrap();
        close_run(&store, &id);
    }
    store
        .db
        .execute("UPDATE links SET retired=1 WHERE link_id='link_A'", [])
        .unwrap();
    drop(store);
    let horizon = DEADLINE + CUTOFF_MS;
    let mut store = open(root.path(), "link_B", "epoch_B", None, horizon);
    attach(
        &store,
        "new_link_run",
        horizon + 30_000,
        4,
        MAX_RUN_MICROUSD,
    );
    assert_eq!(
        store
            .hold_turn("new_link_run", 1, LAUNCH, BODY, horizon, mono)
            .unwrap_err(),
        Fault::BudgetExceeded
    );
    assert!(store.pending_settlements(horizon).unwrap().is_empty());
    close_run(&store, "new_link_run");
    attach(&store, "new_day_run", DAY_MS + 30_000, 4, MAX_RUN_MICROUSD);
    assert_eq!(
        store
            .hold_turn("new_day_run", 1, LAUNCH, BODY, DAY_MS, mono)
            .unwrap()
            .request()
            .body
            .step,
        1
    );
}

#[test]
fn reexec_and_cross_link_cannot_dispatch_or_mutate_other_turns() {
    let (root, mut store) = create();
    let mono = Instant::now();
    attach(&store, "run_A", DEADLINE, 4, MAX_RUN_MICROUSD);
    let held = store.hold_turn("run_A", 1, LAUNCH, BODY, AT, mono).unwrap();
    let lease = store
        .accept_permit(&held, permit(&held, "permit_A", AT + 1_000), AT, mono)
        .unwrap();
    store.consume_turn(&lease, BODY, AT, mono).unwrap();
    drop(store);
    let mut store = open(root.path(), "link_B", "epoch_B", None, AT);
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT, mono)
            .unwrap_err(),
        Fault::NotFound
    );
    assert_eq!(
        store.seal_turn(held.request(), Outcome::Indeterminate, AT),
        Err(Fault::NotFound)
    );
    assert!(store.pending_settlements(AT).unwrap().is_empty());
    drop(store);
    let mut store = open(root.path(), "link_A", "epoch_C", Some("epoch_A"), AT + 100);
    assert_eq!(
        store.consume_turn(&lease, BODY, AT + 100, mono),
        Err(Fault::Stopped)
    );
    assert_eq!(
        store
            .hold_turn("run_A", 1, LAUNCH, BODY, AT + 100, mono)
            .unwrap_err(),
        Fault::Stopped
    );
    let pending = store.pending_settlements(AT + 100).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body.outcome, Outcome::Indeterminate);
    assert_eq!(pending[0].body.permit_id, "permit_A");
    store
        .observe_settlement(&pending[0], &receipt(&pending[0]), AT + 100)
        .unwrap();
    assert_eq!(budget(&store, "run_A"), (true, Some(HOLD_MICROUSD), true));
}

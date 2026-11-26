use super::super::{
    bootstrap::Enrollment,
    tests::{approval, enrollment, new_session, state_root, work_pair},
    wire::FenceReason,
};
use super::*;
use std::{cell::Cell, process::Command};

const NOW: u64 = 1_800_000_000_000;
const QUESTION: &str = "fixture question never persisted by the report ledger";
const EVIDENCE: &str = "fixture tool bytes never copied into the durable receipt ledger";
const CRASH_PATH: &str = "CORRAL_TEST_REPORT_CRASH_ROOT";
const CRASH_BOUNDARY: &str = "CORRAL_TEST_REPORT_CRASH_BOUNDARY";

fn working(root: &Path) -> (Store, String, String) {
    let mut store = Store::enroll(root, &enrollment(), NOW).unwrap();
    let start = wire::params(approval(NOW, "test_request", QUESTION)).unwrap();
    let run = store.start(&start, NOW).unwrap().snapshot.run_id;
    store.bind(&run, 1, NOW).unwrap();
    let launch = store.submit_launch(&run, 1, NOW).unwrap();
    store.attach_launch(&run, 1, &launch, NOW).unwrap();
    (store, run, launch)
}

fn reconnect(root: &Path) -> Store {
    let mut e = enrollment();
    e.prior_channel_epoch = Some(e.channel_epoch.clone());
    e.channel_epoch = "epoch_B".into();
    Store::enroll(root, &e, NOW + 60_001).unwrap()
}

fn args(text: &str, citations: Value) -> Value {
    json!({"text":text,"citations":citations})
}

fn citation(start: u64, end: u64, handle: &str) -> Value {
    json!({"start_byte":start,"end_byte":end,"handle":handle})
}

fn finish<'a>(
    run: &'a str,
    launch: &'a str,
    content: Value,
    evidence: &'a [EvidenceReceipt],
) -> Finish<'a> {
    Finish {
        run_id: run,
        generation: 1,
        launch_id: launch,
        report: Sealed::new(content).unwrap(),
        evidence,
    }
}

fn key(o: &Offer) -> Value {
    json!({"run_id":o.run_id,"generation":o.generation,"report_id":o.report_id})
}

fn ack(o: &Offer) -> Value {
    json!({"run_id":o.run_id,"generation":o.generation,"report_id":o.report_id,"bytes":o.bytes,"sha256":o.sha256})
}

#[test]
fn report_publication_binds_correlated_receipts_and_ack_retains_one_copy() {
    let dir = state_root();
    let (mut store, run, launch) = working(dir.path());
    let call_args = &"a".repeat(64);
    let payload = json!({"status":"ok","citation":"approved","text":EVIDENCE});
    let evidence = [EvidenceReceipt::accepted(
        "broker_invocation",
        "read_evidence",
        call_args,
        &payload,
        None,
    )
    .unwrap()];
    let cite = citation(0, 2, "approved");
    let second = citation(3, 4, "approved");
    let content = args("é !", json!([second, cite]));
    for invalid in [
        json!({"path":"elsewhere","text":"x","citations":[]}),
        args("é", json!([citation(1, 2, "approved")])),
        args("é", json!([cite, cite])),
        args(&"\n".repeat(40_000), json!([])), // UTF-8 text fits; sealed escaped bytes do not.
    ] {
        assert!(matches!(Sealed::new(invalid), Err(Fault::Invalid)));
    }
    let before = store.read(&run).unwrap();
    assert_eq!(
        store.finish_report(finish(&run, &launch, content.clone(), &[]), || Ok(NOW)),
        Err(Fault::Invalid)
    );
    assert_eq!(
        store.finish_report(
            Finish {
                generation: 2,
                ..finish(&run, &launch, content.clone(), &evidence)
            },
            || Ok(NOW)
        ),
        Err(Fault::StaleGeneration)
    );
    assert_eq!(
        store.finish_report(
            finish(&run, "other_launch", content.clone(), &evidence),
            || Ok(NOW)
        ),
        Err(Fault::Stopped)
    );
    let offer = store
        .finish_report(finish(&run, &launch, content, &evidence), || Ok(NOW))
        .unwrap();
    let sealed = wire::canonical(
        &json!({"v":1,"kind":"investigation_report","text":"é !","citations":[cite,second]}),
    )
    .unwrap();
    assert_eq!(offer.sha256, wire::sha256(&sealed));
    assert_eq!(offer.bytes, sealed.len());
    let snapshot = store.read(&run).unwrap();
    assert_eq!(snapshot.state, "finished");
    assert_eq!(snapshot.offer, Some(offer.clone()));
    assert_eq!(snapshot.seq, before.seq + 1);
    assert_eq!(store.stop(&run, 1, FenceReason::Stopped).unwrap(), snapshot);
    store.confirm_teardown(&run, 1, &launch, None).unwrap();
    assert_eq!(store.read(&run).unwrap(), snapshot);
    let same = || finish(&run, &launch, args("é !", json!([cite, second])), &[]);
    assert_eq!(
        store.finish_report(same(), || panic!("no new publication")),
        Ok(offer.clone())
    );
    assert_eq!(
        store.finish_report(
            finish(&run, &launch, args("different", json!([])), &[]),
            || Ok(NOW)
        ),
        Err(Fault::Conflict)
    );
    let (work, _peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    let read_report = json!({"offer":offer,"report_b64":STANDARD.encode(&sealed)});
    for _ in 0..2 {
        assert_eq!(
            session.apply("fetch_report", key(&offer), (NOW, NOW)),
            Ok(read_report.clone())
        );
    }
    let mut bad = ack(&offer);
    bad["bytes"] = json!(offer.bytes + 1);
    assert_eq!(
        session.apply("ack_report", bad, (NOW, NOW)),
        Err(Fault::Conflict)
    );
    let mut bad = key(&offer);
    bad["generation"] = json!(2);
    assert_eq!(
        session.apply("fetch_report", bad, (NOW, NOW)),
        Err(Fault::NotFound)
    );
    assert_eq!(
        session.apply("remove_report", key(&offer), (NOW, NOW)),
        Err(Fault::Unsupported)
    );
    for _ in 0..2 {
        assert_eq!(
            session.apply("ack_report", ack(&offer), (NOW, NOW)),
            Ok(json!({"offer":offer,"acked":true}))
        );
    }
    assert_eq!(
        session.apply("fetch_report", key(&offer), (NOW, NOW)),
        Ok(read_report.clone())
    );
    let acked = store.read(&run).unwrap();
    assert!(acked.acked);
    assert_eq!(acked.seq, snapshot.seq + 1);
    store.disconnected().unwrap();
    drop(store);
    let mut store = reconnect(dir.path());
    assert_eq!(store.read(&run).unwrap(), acked);
    assert_eq!(
        store.fetch_report(wire::params(key(&offer)).unwrap()),
        Ok(read_report)
    );
    let receipt: String = store
        .db
        .query_row("SELECT receipt FROM reports", [], |r| r.get(0))
        .unwrap();
    let receipt: Value = wire::decode(&receipt.into_bytes()).unwrap();
    assert!(wire::valid_id(
        receipt["finish"]["invocation_id"].as_str().unwrap()
    ));
    assert_eq!(receipt["evidence"][0]["invocation_id"], "broker_invocation");
    assert_eq!(receipt["evidence"][0]["args_digest"], *call_args);
    assert_eq!(
        receipt["evidence"][0]["response_digest"],
        wire::digest(&payload).unwrap()
    );
    let db = fs::read(dir.path().join("lite-analyst.sqlite3")).unwrap();
    for discarded in [QUESTION.as_bytes(), EVIDENCE.as_bytes(), sealed.as_slice()] {
        assert!(!db.windows(discarded.len()).any(|w| w == discarded));
    }
    assert_eq!(fs::read_dir(&store.reports).unwrap().count(), 1);
    let blob = store.reports.join(&offer.report_id);
    assert_eq!(fs::read(&blob).unwrap(), sealed);
    assert_eq!(fs::metadata(&blob).unwrap().mode() & 0o777, 0o400);
    assert_eq!(
        store.finish_report(same(), || panic!("replay cannot publish")),
        Ok(offer.clone())
    );
    store.remove_report(&run).unwrap();
    store.remove_report(&run).unwrap();
    assert!(!blob.exists());
    let deleted = store.read(&run).unwrap();
    assert_eq!(deleted.state, "finished");
    assert!(deleted.offer.is_none() && !deleted.acked);
    assert_eq!(deleted.seq, acked.seq + 1);
    assert_eq!(
        store.finish_report(same(), || panic!("tombstone prevents reuse")),
        Err(Fault::NotFound)
    );
    assert_eq!(
        store.ack_report(wire::params(ack(&offer)).unwrap()),
        Err(Fault::NotFound)
    );
    assert_eq!(
        store.fetch_report(wire::params(key(&offer)).unwrap()),
        Err(Fault::NotFound)
    );
    assert!(
        store
            .db
            .query_row("SELECT receipt IS NULL FROM reports", [], |r| r
                .get::<_, bool>(0))
            .unwrap()
    );
    let original = wire::params(approval(NOW, "test_request", QUESTION)).unwrap();
    assert_eq!(
        store.start(&original, NOW + 60_001).unwrap().snapshot,
        deleted
    );
}

#[test]
fn fence_or_expiry_before_publication_has_no_report() {
    for cause in ["cancel", "expire", "disconnect"] {
        let dir = state_root();
        let (mut store, run, launch) = working(dir.path());
        let checks = Cell::new(0);
        let (work, peer, _life) = work_pair();
        if cause == "cancel" {
            assert_eq!(
                store.stop(&run, 1, FenceReason::Stopped).unwrap().state,
                "stop_requested"
            );
        }
        let reports = store.reports.clone();
        let result = store.finish_report(
            finish(&run, &launch, args("limitations", json!([])), &[]),
            || {
                let checks = checks.replace(checks.get() + 1);
                if checks == 1 {
                    assert_eq!(fs::read_dir(&reports).unwrap().count(), 1);
                    if cause == "expire" {
                        return Ok(NOW + 60_000);
                    }
                    peer.disconnect();
                }
                work.check_live()?;
                Ok(NOW)
            },
        );
        assert_eq!(
            result,
            Err(match cause {
                "expire" => Fault::Expired,
                "disconnect" => Fault::Unavailable,
                _ => Fault::Stopped,
            })
        );
        assert_eq!(checks.get(), if cause == "cancel" { 0 } else { 2 });
        let stopped = store.stop(&run, 1, FenceReason::Stopped).unwrap();
        assert!(stopped.offer.is_none());
        assert_eq!(stopped.state, "stop_requested");
        assert_eq!(fs::read_dir(&reports).unwrap().count(), 0);
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM reports", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn foreign_links_and_corrupt_or_missing_blobs_cannot_deliver() {
    let dir = state_root();
    let (mut store, run, launch) = working(dir.path());
    let offer = store
        .finish_report(
            finish(&run, &launch, args("limitations", json!([])), &[]),
            || Ok(NOW),
        )
        .unwrap();
    let path = store.reports.join(&offer.report_id);
    fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, vec![b'x'; offer.bytes]).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o400)).unwrap();
    assert_eq!(
        store.fetch_report(wire::params(key(&offer)).unwrap()),
        Err(Fault::Unavailable)
    );
    assert_eq!(
        store.ack_report(wire::params(ack(&offer)).unwrap()),
        Err(Fault::Unavailable)
    );
    assert!(!store.read(&run).unwrap().acked);
    fs::remove_file(path).unwrap();
    assert_eq!(
        store.fetch_report(wire::params(key(&offer)).unwrap()),
        Err(Fault::Unavailable)
    );
    drop(store);
    let foreign = Enrollment {
        link_id: "foreign".into(),
        ..enrollment()
    };
    let mut store = Store::enroll(dir.path(), &foreign, NOW).unwrap();
    assert_eq!(store.read(&run), Err(Fault::NotFound));
    assert_eq!(
        store.fetch_report(wire::params(key(&offer)).unwrap()),
        Err(Fault::NotFound)
    );
    assert_eq!(
        store.ack_report(wire::params(ack(&offer)).unwrap()),
        Err(Fault::NotFound)
    );
    assert_eq!(store.remove_report(&run), Err(Fault::NotFound));
    drop(store);
    let mut store = reconnect(dir.path());
    store.retire().unwrap();
    assert_eq!(
        store.fetch_report(wire::params(key(&offer)).unwrap()),
        Err(Fault::Stopped)
    );
    assert_eq!(
        store.ack_report(wire::params(ack(&offer)).unwrap()),
        Err(Fault::Stopped)
    );
}

#[test]
fn abrupt_exit_around_report_commit_leaves_only_the_durable_winner() {
    if let Some(root) = std::env::var_os(CRASH_PATH) {
        let (mut store, run, launch) = working(Path::new(&root));
        let before = std::env::var(CRASH_BOUNDARY).unwrap() == "before";
        let checks = Cell::new(0);
        store
            .finish_report(
                finish(&run, &launch, args("limitations", json!([])), &[]),
                || {
                    if checks.replace(checks.get() + 1) == 1 && before {
                        std::process::exit(53); // File sync and INSERT done; no destructor/rollback ran.
                    }
                    Ok(NOW)
                },
            )
            .unwrap();
        assert!(!before);
        std::process::exit(53); // Terminal commit done; no local or remote receipt was handed off.
    }
    for boundary in ["before", "after"] {
        let dir = state_root();
        let module = module_path!().split_once("::").unwrap().1;
        let test =
            format!("{module}::abrupt_exit_around_report_commit_leaves_only_the_durable_winner");
        let outcome = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test])
            .env(CRASH_PATH, dir.path())
            .env(CRASH_BOUNDARY, boundary)
            .output()
            .unwrap();
        assert_eq!(
            outcome.status.code(),
            Some(53),
            "{}",
            String::from_utf8_lossy(&outcome.stdout)
        );
        assert_eq!(fs::read_dir(dir.path().join("reports")).unwrap().count(), 1);
        let store = reconnect(dir.path());
        let run: String = store
            .db
            .query_row(
                "SELECT run_id FROM runs WHERE launch_phase='cleaned'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let snapshot = store.read(&run).unwrap();
        if boundary == "before" {
            assert_eq!(fs::read_dir(&store.reports).unwrap().count(), 0);
            assert_eq!(snapshot.state, "cancelled");
            assert!(snapshot.offer.is_none());
        } else {
            assert_eq!(snapshot.state, "finished");
            let offer = snapshot.offer.unwrap();
            assert_eq!(fs::read_dir(&store.reports).unwrap().count(), 1);
            assert!(
                store
                    .fetch_report(wire::params(key(&offer)).unwrap())
                    .is_ok()
            );
        }
    }
}

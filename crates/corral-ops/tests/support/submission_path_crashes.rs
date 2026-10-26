//! Child-process driver for the crate-private real automatic and owner entries.
use super::*;
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Envelope, Observation, ObserveCursor, Readiness,
    SendOutcome, SessionBinding,
};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
    process::CommandExt,
};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The normal delivery pipeline with only its transport and observation boundary faked.
struct RepairDelivery {
    pane: String,
    observations: std::cell::RefCell<Vec<Observation>>,
}

impl DeliveryAdapter for RepairDelivery {
    fn harness(&self) -> &'static str {
        "submission-fixture"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: false,
            admission: true,
            non_waking: false,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
            send_budget: None,
            transcript_reader: true,
        }
    }
    fn readiness(&self, _: &crate::Ctx, _: &corral_core::inbox::Recipient) -> Result<Readiness> {
        Ok(Readiness::Native)
    }
    fn bind(&self, _: &crate::Ctx, _: &corral_core::inbox::Recipient) -> Result<BindOutcome> {
        Ok(BindOutcome::Bound(SessionBinding {
            session: self.pane.clone(),
            endpoint: None,
            transcript: None,
        }))
    }
    fn send(&self, ctx: &crate::Ctx, _: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        let (key, _) = envelope.idempotency_key.rsplit_once("@env").unwrap();
        let inbox = corral_core::inbox::Inbox::new(
            &ctx.store,
            corral_core::inbox::Recipient::parse_key(key).unwrap(),
        )
        .unwrap();
        let payload = inbox.payload(envelope).unwrap();
        assert!(payload.contains("repair regression"));
        let delivery = ctx.herdr.agent_prompt(&self.pane, &payload).unwrap();
        assert!(delivery.submitted, "{delivery:?}");
        self.observations.borrow_mut().push(Observation::Delivered {
            envelope: envelope.number,
            members: envelope.members.clone(),
            detail: serde_json::json!({"source":"fake transport observation"}),
        });
        SendOutcome::Accepted
    }
    fn observe(
        &self,
        _: &crate::Ctx,
        _: &SessionBinding,
        _: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        Ok(std::mem::take(&mut *self.observations.borrow_mut()))
    }
}

struct Process(Child);
impl Process {
    fn kill(&mut self) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.0.id()).unwrap()).unwrap();
        rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).unwrap();
        assert!(!self.0.wait().unwrap().success());
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.kill();
        }
    }
}

fn stop(point: &str) {
    if std::env::var("CORRAL_SUBMISSION_STOP").unwrap() == point {
        let mut stream =
            UnixStream::connect(std::env::var_os("CORRAL_SUBMISSION_SOCKET").unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        writeln!(stream, "{point}").unwrap();
        let mut release = [0];
        stream.read_exact(&mut release).unwrap();
        assert_eq!(release, [b'R']);
    }
}

fn path(name: &str) -> Path {
    match name {
        "Worker" => Path::Worker,
        "Planner" => Path::Planner,
        "RepairLegacy" => Path::RepairLegacy,
        "RepairInbox" => Path::RepairInbox,
        "PlanReviewPane" => Path::PlanReviewPane,
        "PlanReviewPm" => Path::PlanReviewPm,
        "AcceptPlan" => Path::AcceptPlan,
        "ReviewerPane" => Path::ReviewerPane,
        "ReviewerPm" => Path::ReviewerPm,
        "Gate" => Path::Gate,
        other => panic!("unknown child path {other}"),
    }
}

#[test]
fn submission_paths_child_entry() {
    let Some(root) = std::env::var_os("CORRAL_SUBMISSION_CHILD_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let store = corral_core::Store::open(&root)
        .unwrap()
        .with_submission_barrier(Arc::new(stop));
    let ctx = crate::Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(root.join("engine/herdr"), Some("corral-test".into())),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    let id = std::env::var("CORRAL_SUBMISSION_TASK").unwrap();
    let automatic = std::env::var("CORRAL_SUBMISSION_AUTOMATIC").unwrap() == "true";
    let name = std::env::var("CORRAL_SUBMISSION_PATH").unwrap();
    if name == "aggregate" {
        crate::plan_accept(
            &ctx,
            &id,
            &std::env::var("CORRAL_SUBMISSION_ATTEMPT").unwrap(),
            automatic,
        )
        .unwrap();
    } else {
        path(&name).enter(&ctx, &id, automatic).unwrap();
    }
}

struct Run<'a> {
    fx: &'a Fx,
    id: &'a str,
    name: String,
    kind: OperatingActionKind,
    automatic: bool,
    attempt: &'a str,
    effects: (usize, usize, usize),
}

fn crash(run: Run<'_>, point: &str) {
    let Run {
        fx,
        id,
        name,
        kind,
        automatic,
        attempt,
        effects,
    } = run;
    std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/submission_herdr.py"),
        fx.bin.join("herdr"),
    )
    .unwrap();
    std::fs::set_permissions(fx.bin.join("herdr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let socket = fx.bin.join("submission.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let before = counts(fx);
    let before_records = authorizations(&fx.ctx, id, kind);
    let mut child = Process(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "submission_paths_tests::crashes::submission_paths_child_entry",
                "--nocapture",
            ])
            .env("CORRAL_SUBMISSION_CHILD_ROOT", fx.temp.path())
            .env("CORRAL_SUBMISSION_TASK", id)
            .env("CORRAL_SUBMISSION_PATH", &name)
            .env("CORRAL_SUBMISSION_AUTOMATIC", automatic.to_string())
            .env("CORRAL_SUBMISSION_ATTEMPT", attempt)
            .env("CORRAL_SUBMISSION_STOP", point)
            .env("CORRAL_SUBMISSION_SOCKET", &socket)
            .env(
                "CORRAL_SUBMISSION_ENGINE_FIXTURE",
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/operating_launch/herdr.py"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let mut fds = [PollFd::new(&listener, PollFlags::IN)];
    assert_eq!(
        poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 45,
                tv_nsec: 0
            })
        )
        .unwrap(),
        1,
        "{name} auto={automatic} never reached {point}: {:?}",
        child.0.try_wait().unwrap()
    );
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut arrived = String::new();
    BufReader::new(&stream).read_line(&mut arrived).unwrap();
    assert_eq!(arrived.trim(), point);
    child.kill();
    drop(stream);
    let database = corral_core::sqlite::SqliteBackend::open(
        &fx.temp.path().join(".corral"),
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let integrity = corral_core::backend::StoreBackend::check(&database).unwrap();
    assert!(integrity.is_ok(), "{name} {point}: {integrity:?}");
    let records = authorizations(&fx.fresh(), id, kind);
    let disposition = match point {
        "prepare_returned" | "inbox_reserved" => {
            // An unchanged definite refusal remains until the atomic submission,
            // including automatic retries under the same strict policy bytes.
            before_records
                .iter()
                .find(|record| {
                    matches!(
                        record.disposition,
                        SubmissionDisposition::AuthorizationRefused { .. }
                    )
                })
                .map_or(SubmissionDisposition::Prepared, |record| {
                    record.disposition.clone()
                })
        }
        "submitted_committed" => SubmissionDisposition::Submitted,
        _ => SubmissionDisposition::Submitting,
    };
    let record = records
        .iter()
        .find(|record| record.disposition == disposition)
        .unwrap_or_else(|| panic!("{name} {point}: {records:?}"));
    let committed = !matches!(point, "prepare_returned" | "inbox_reserved");
    let task = fx.ctx.store.get(id).unwrap();
    match name.as_str() {
        "Worker" => {
            assert_eq!(
                task.launch.as_ref().unwrap().stage,
                if committed {
                    corral_core::LaunchStage::Submitting
                } else {
                    corral_core::LaunchStage::Pane
                }
            );
        }
        "RepairLegacy" | "RepairInbox" => {
            assert_eq!(
                task.status,
                if committed {
                    Status::Running
                } else {
                    Status::Review
                }
            );
            assert_eq!(
                task.current_run().unwrap().pending_reply.is_some(),
                committed
            );
            if name == "RepairInbox" {
                let inbox = corral_core::inbox::Inbox::new(
                    &fx.ctx.store,
                    corral_core::inbox::Recipient::Run {
                        task: id.into(),
                        run: "R1".into(),
                    },
                )
                .unwrap();
                assert_eq!(
                    inbox.messages().unwrap().len(),
                    usize::from(committed),
                    "inbox reservation and run reopening must commit together"
                );
                assert_eq!(
                    inbox.records("messages").unwrap().len(),
                    usize::from(committed)
                );
                assert_eq!(
                    inbox.pending_messages().unwrap().len(),
                    usize::from(committed)
                );
            }
        }
        "AcceptPlan" => {
            let approval = task.plans.last().unwrap().approval.as_ref();
            assert_eq!(approval.is_some(), committed);
            if let Some(approval) = approval {
                assert_eq!(
                    approval.authorization.as_deref(),
                    Some(record.authorization.id.as_str())
                );
            }
        }
        "aggregate" => {
            let acceptance = task.aggregate_attempts.last().unwrap().acceptance.as_ref();
            assert_eq!(acceptance.is_some(), committed);
            if let Some(acceptance) = acceptance {
                assert_eq!(acceptance.authorization, record.authorization.id);
                assert_eq!(
                    acceptance.state,
                    corral_core::AggregateLandingState::Prepared
                );
            }
        }
        _ => {}
    }
    let expected = if matches!(
        point,
        "effect_returned" | "send_returned" | "submitted_committed"
    ) {
        effects
    } else {
        (0, 0, 0)
    };
    let after = counts(fx);
    assert_eq!(
        (after.0 - before.0, after.1 - before.1, after.2 - before.2),
        expected,
        "{name} auto={automatic} {point}"
    );
    if committed {
        let error = fx
            .ctx
            .store
            .submit_operating_action(
                &record.authorization,
                &record.authorization.action,
                |_, _| panic!("crash replay prepared"),
                || -> Result<()> { panic!("crash replay sent") },
            )
            .unwrap_err();
        assert!(error.to_string().contains("refusing replay"), "{error:#}");
    }
    assert_eq!(counts(fx), after);
    // The barrier wrapper's environment belongs only to the dead child. Recovery uses the
    // same fake transport without an armed barrier, never an inherited real herdr client.
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/operating_launch/herdr.py"),
        fx.bin.join("herdr"),
    )
    .unwrap();
    std::fs::set_permissions(fx.bin.join("herdr"), std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn submission_paths_inbox_reservation_rows_survive_sqlite_crashes() {
    for point in ["inbox_reserved", "authorization_committed"] {
        let (mut fx, id) = Path::RepairInbox.setup();
        fx.grant(&[&id]);
        crash(
            Run {
                fx: &fx,
                id: &id,
                name: "RepairInbox".into(),
                kind: Path::RepairInbox.kind(),
                automatic: true,
                attempt: "",
                effects: Path::RepairInbox.effects(),
            },
            point,
        );
    }
}

#[test]
fn submission_paths_all_automatic_and_owner_entries_survive_sqlite_crashes() {
    for candidate in [
        Path::Worker,
        Path::Planner,
        Path::RepairLegacy,
        Path::RepairInbox,
        Path::PlanReviewPane,
        Path::PlanReviewPm,
        Path::AcceptPlan,
        Path::ReviewerPane,
        Path::ReviewerPm,
        Path::Gate,
    ] {
        for automatic in [false, true] {
            if !automatic
                && matches!(
                    candidate,
                    Path::RepairLegacy | Path::RepairInbox | Path::AcceptPlan
                )
            {
                continue;
            }
            for point in [
                "prepare_returned",
                "authorization_committed",
                "effect_entered",
                "effect_returned",
                "send_returned",
                "submitted_committed",
            ] {
                if matches!(candidate, Path::AcceptPlan) && point.starts_with("effect_") {
                    continue;
                }
                let (mut fx, id) = candidate.setup();
                if automatic {
                    if matches!(candidate, Path::PlanReviewPm) {
                        fx.ctx.store.config.review.reviewer = Some("pm".into());
                        fx.ctx.store.save_config().unwrap();
                    }
                    fx.grant(&[&id]);
                }
                crash(
                    Run {
                        fx: &fx,
                        id: &id,
                        name: format!("{candidate:?}"),
                        kind: candidate.kind(),
                        automatic,
                        attempt: "",
                        effects: candidate.effects(),
                    },
                    point,
                );
                if point == "submitted_committed" {
                    let before_recovery = counts(&fx);
                    if matches!(candidate, Path::RepairInbox) {
                        let recipient = corral_core::inbox::Recipient::Run {
                            task: id.clone(),
                            run: "R1".into(),
                        };
                        let delivery = RepairDelivery {
                            pane: fx
                                .ctx
                                .store
                                .get(&id)
                                .unwrap()
                                .current_run()
                                .unwrap()
                                .herdr
                                .as_ref()
                                .unwrap()
                                .pane_id
                                .clone(),
                            observations: Default::default(),
                        };
                        crate::delivery::owner::kick_with(&fx.fresh(), &recipient, &delivery)
                            .unwrap();
                        crate::delivery::owner::kick_with(&fx.fresh(), &recipient, &delivery)
                            .unwrap();
                        let after_recovery = counts(&fx);
                        assert_eq!(after_recovery.0, before_recovery.0);
                        assert_eq!(after_recovery.1, before_recovery.1 + 1);
                        assert_eq!(after_recovery.2, before_recovery.2);
                        let inbox =
                            corral_core::inbox::Inbox::new(&fx.ctx.store, recipient).unwrap();
                        let messages = inbox.messages().unwrap();
                        assert_eq!(messages.len(), 1);
                        assert!(
                            inbox
                                .receipt(
                                    messages[0].seq,
                                    corral_core::inbox::ReceiptLevel::Observed
                                )
                                .unwrap()
                                .is_some()
                        );
                    } else if matches!(candidate, Path::AcceptPlan) {
                        assert!(fx.ctx.store.get(&id).unwrap().plan.is_none());
                        crate::plan_pilot::advance_all(&fx.fresh(), fixture::now()).unwrap();
                        let recovered = fx.ctx.store.get(&id).unwrap();
                        assert_eq!(recovered.plan.as_deref(), Some(format!("{id}@P1").as_str()));
                        assert_eq!(counts(&fx), before_recovery);
                        assert_eq!(
                            authorizations(&fx.ctx, &id, OperatingActionKind::AcceptPlan).len(),
                            1,
                            "the next plan-pilot pass must apply the committed acceptance, not resubmit"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn submission_paths_recovered_auxiliaries_survive_sqlite_crashes() {
    for candidate in [Path::Gate, Path::ReviewerPane, Path::ReviewerPm] {
        for automatic in [false, true] {
            for point in [
                "prepare_returned",
                "authorization_committed",
                "effect_entered",
                "effect_returned",
                "send_returned",
                "submitted_committed",
            ] {
                let (fx, id) = recovered_setup(candidate, automatic);
                crash(
                    Run {
                        fx: &fx,
                        id: &id,
                        name: format!("{candidate:?}"),
                        kind: candidate.kind(),
                        automatic,
                        attempt: "",
                        effects: candidate.effects(),
                    },
                    point,
                );
            }
        }
    }
}

#[test]
fn submission_paths_aggregate_acceptance_survives_sqlite_crashes() {
    for code in [false, true] {
        for automatic in [false, true] {
            for point in [
                "prepare_returned",
                "authorization_committed",
                "effect_entered",
                "effect_returned",
                "send_returned",
                "submitted_committed",
            ] {
                if !code && point.starts_with("effect_") {
                    continue;
                }
                let (fx, id, attempt, before) = aggregate::scenario(code, automatic);
                crash(
                    Run {
                        fx: &fx,
                        id: &id,
                        name: "aggregate".into(),
                        kind: OperatingActionKind::AcceptAggregate,
                        automatic,
                        attempt: &attempt,
                        effects: (0, 0, 0),
                    },
                    point,
                );
                let repo = fx.ctx.store.project_path("app").unwrap();
                let main = fixture::git(&repo, &["rev-parse", "main"]);
                let landed = code
                    && matches!(
                        point,
                        "effect_returned" | "send_returned" | "submitted_committed"
                    );
                assert_eq!(main != before, landed, "code={code} {point}");
                if point == "submitted_committed" {
                    let before_recovery = counts(&fx);
                    let accepted =
                        crate::plan_accept(&fx.fresh(), &id, &attempt, automatic).unwrap();
                    assert_eq!(accepted.state, corral_core::AggregateLandingState::Landed);
                    assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::Done);
                    assert_eq!(fixture::git(&repo, &["rev-parse", "main"]), main);
                    assert_eq!(counts(&fx), before_recovery);
                }
            }
        }
    }
}

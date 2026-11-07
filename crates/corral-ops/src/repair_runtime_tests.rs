use super::*;
use corral_core::{Profile, RepairPolicy, Status};
use serde_json::json;
#[path = "repair_runtime_cowork_tests.rs"]
mod cowork_cycles;

struct FixedClock;
impl FixedClock {
    fn new() -> Self {
        corral_core::clock::pin_thread(Some("2026-11-04T01:00:00Z".parse().unwrap()));
        Self
    }
}
impl Drop for FixedClock {
    fn drop(&mut self) {
        corral_core::clock::pin_thread(None);
    }
}

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("app")).unwrap();
    let mut store = Store::init(root.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    store.config.profiles.insert(
        "general".into(),
        serde_json::from_value::<Profile>(json!({
            "harness":"generic", "escalate_to":"deep"
        }))
        .unwrap(),
    );
    store.config.profiles.insert(
        "deep".into(),
        serde_json::from_value::<Profile>(json!({"harness":"generic"})).unwrap(),
    );
    store.config.routing.repair = Some(RepairPolicy::approved_starter_policy());
    store.save_config().unwrap();
    let task = store
        .create_initialized("Repair", Some("app"), Status::Review, "", |t| {
            t.runs
                .push(serde_json::from_value(json!({"id":"R1","agent":"general",
            "worktree":"/not-used","branch":"test","base":"main","base_sha":"abc",
            "started":"2026-11-04T00:00:00Z","outcome":"done","route":{"source":"defaults"}}))?);
            Ok(())
        })
        .unwrap();
    (
        root,
        crate::resources::FixedResourceProbe::ctx(store),
        task.id,
    )
}

fn report(id: &str, test: &str) -> FindingReport {
    FindingReport {
        observation_id: id.into(),
        identity: FailureIdentity {
            criterion_or_test: test.into(),
            affected_contract_or_location: "transaction.rs".into(),
            failure_code: "rollback".into(),
        },
        observed_failure: "commit escaped rollback".into(),
        attempted_fixes: Vec::new(),
        resolution: FindingResolution::Unresolved,
    }
}

fn begin(ctx: &Ctx, id: &str, attempt: &str, profile: &str, availability: bool) -> Result<()> {
    ctx.store.update_head(id, |task| {
        let run = task.current_run_mut().unwrap();
        run.agent = profile.into();
        run.model = Some(format!("{profile}-model"));
        start(&ctx.store, &*task, attempt, profile, availability)
    })?;
    Ok(())
}

#[test]
fn durable_repairs_are_once_only_and_survive_switches_and_new_findings() {
    let _clock = FixedClock::new();
    let (_root, ctx, id) = fixture();
    begin(&ctx, &id, "launch:R1", "general", false).unwrap();
    begin(&ctx, &id, "launch:R1", "general", false).unwrap();
    let first = repair_finding(&ctx, &id, report("finding-1", "rollback-test")).unwrap();
    assert_eq!(
        first
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        0
    );
    let refused = ctx.store.update_head(&id, |task| {
        start(&ctx.store, &*task, "refused-prepare", "general", false)?;
        anyhow::bail!("later preparation refused")
    });
    assert!(refused.is_err());
    assert_eq!(repair_status(&ctx, &id).unwrap().unwrap(), first);
    for (attempt, profile) in [
        ("repair1", "general"),
        ("repair2", "general"),
        ("repair3", "deep"),
        ("repair4", "deep"),
    ] {
        begin(&ctx, &id, attempt, profile, false).unwrap();
        let before = repair_status(&ctx, &id).unwrap().unwrap();
        begin(&ctx, &id, attempt, profile, false).unwrap();
        assert_eq!(repair_status(&ctx, &id).unwrap().unwrap(), before);
        let reopened = Ctx::from_store(Store::open(ctx.store.root()).unwrap());
        assert_eq!(repair_status(&reopened, &id).unwrap().unwrap(), before);
        repair_finding(
            &ctx,
            &id,
            report(&format!("after-{attempt}"), "rollback-test"),
        )
        .unwrap();
    }
    let state = repair_status(&ctx, &id).unwrap().unwrap();
    assert_eq!(state.transition().unwrap(), RepairTransition::Diagnose);
    assert!(begin(&ctx, &id, "fifth", "deep", false).is_err());
    // Verified progress and a different test do not create a new allowance.
    let mut resolved = report("resolved-first", "rollback-test");
    resolved.resolution = FindingResolution::Resolved {
        evidence: vec!["gate:rollback-test:pass".into()],
    };
    repair_finding(&ctx, &id, resolved).unwrap();
    let new = repair_finding(&ctx, &id, report("finding-2", "isolation-test")).unwrap();
    assert_eq!(
        new.active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        4
    );
    assert_eq!(new.active().unwrap().history.findings().len(), 2);
    assert!(begin(&ctx, &id, "renamed-profile", "general", false).is_err());
    let mut edited = Store::open(ctx.store.root()).unwrap();
    edited.config.routing.repair = None;
    edited.save_config().unwrap();
    assert!(begin(&ctx, &id, "config-reset", "deep", false).is_err());
    assert_eq!(
        repair_status(&ctx, &id)
            .unwrap()
            .unwrap()
            .active()
            .unwrap()
            .history
            .policy(),
        &RepairPolicy::approved_starter_policy()
    );
    // Adopting policy over unaccounted legacy corrections must not authorize a free
    // initial launch, even if the old run is still the only run in the task.
    let (_legacy_root, legacy, legacy_id) = fixture();
    legacy
        .store
        .update_head(&legacy_id, |task| {
            task.rounds = 1;
            Ok(())
        })
        .unwrap();
    assert!(begin(&legacy, &legacy_id, "launch:R1", "general", false).is_err());
}

#[test]
fn exact_reports_keep_identity_and_evidence_while_outages_do_not_spend_repairs() {
    let _clock = FixedClock::new();
    let (_root, ctx, id) = fixture();
    let first = repair_finding(&ctx, &id, report("first", "rollback-test")).unwrap();
    assert_eq!(
        repair_finding(&ctx, &id, report("first", "rollback-test")).unwrap(),
        first
    );
    begin(&ctx, &id, "fix1", "general", false).unwrap();
    let mut repeat = report("repeat", "rollback-test");
    repeat.attempted_fixes.push(AttemptedFix {
        attempt: RepairAttemptId("fix1".into()),
        change: "retain transaction handle".into(),
        evidence: vec!["commit:a".into()],
    });
    let repeated = repair_finding(&ctx, &id, repeat.clone()).unwrap();
    assert_eq!(
        repeated.active().unwrap().history.findings()[0].finding_id,
        first.active().unwrap().history.findings()[0].finding_id
    );
    assert!(matches!(
        repeated.active().unwrap().history.findings()[0].matching,
        FindingMatch::Exact { .. }
    ));
    assert_eq!(repair_finding(&ctx, &id, repeat).unwrap(), repeated);
    begin(&ctx, &id, "outage", "general", true).unwrap();
    let final_state = repair_status(&ctx, &id).unwrap().unwrap();
    let counts = final_state.active().unwrap().history.counters().unwrap();
    assert_eq!(
        (counts.coding_repairs, counts.availability_recoveries),
        (1, 1)
    );
    let head = ctx.store.head(&id).unwrap();
    prepare_continuation(&ctx.store, &head, "R1", true).unwrap();
    for run_id in ["R2", "R3"] {
        ctx.store
            .update(&id, |task| {
                let mut run = task.current_run().unwrap().clone();
                run.id = run_id.into();
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        ctx.store
            .update_head(&id, |task| start_launch(&ctx.store, &*task))
            .unwrap();
        assert!(
            ctx.store
                .record::<Continuation>("repair-continuation", &id)
                .unwrap()
                .is_none()
        );
    }
    // Only R2 is the linked availability continuation. R3 is an ordinary corrective
    // launch; a retained marker must not silently exempt all future coding attempts.
    let counts = repair_status(&ctx, &id)
        .unwrap()
        .unwrap()
        .active()
        .unwrap()
        .history
        .counters()
        .unwrap();
    assert_eq!(
        (counts.coding_repairs, counts.availability_recoveries),
        (2, 2)
    );
}

#[test]
fn pins_and_unavailable_stronger_profiles_block_without_spending_their_allowance() {
    let _clock = FixedClock::new();
    let (_root, ctx, id) = fixture();
    repair_finding(&ctx, &id, report("first", "rollback-test")).unwrap();
    begin(&ctx, &id, "fix1", "general", false).unwrap();
    begin(&ctx, &id, "fix2", "general", false).unwrap();
    let head = ctx
        .store
        .update_head(&id, |task| {
            task.profile = Some("general".into());
            Ok(())
        })
        .unwrap();
    let ledger = repair_status(&ctx, &id).unwrap().unwrap();
    assert_eq!(
        transition(&ctx, &head, &ledger, false).unwrap(),
        RepairTransition::Diagnose
    );
    assert_eq!(
        repair_status(&ctx, &id)
            .unwrap()
            .unwrap()
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .stronger_profile_repairs,
        0
    );
    assert!(begin(&ctx, &id, "invented", "other", false).is_err());

    // Removed and missing stronger configurations do not donate their unused budget.
    for missing in [false, true] {
        let (_root, mut ctx, id) = fixture();
        if missing {
            ctx.store
                .config
                .profiles
                .get_mut("general")
                .unwrap()
                .escalate_to = None;
            ctx.store.save_config().unwrap();
        }
        repair_finding(&ctx, &id, report("first", "rollback-test")).unwrap();
        begin(&ctx, &id, "fix1", "general", false).unwrap();
        begin(&ctx, &id, "fix2", "general", false).unwrap();
        if !missing {
            ctx.store.config.profiles.remove("deep");
            ctx.store
                .config
                .profiles
                .get_mut("general")
                .unwrap()
                .escalate_to = None;
            ctx.store.save_config().unwrap();
        }
        let head = ctx.store.head(&id).unwrap();
        let ledger = repair_status(&ctx, &id).unwrap().unwrap();
        assert_eq!(
            transition(&ctx, &head, &ledger, false).unwrap(),
            RepairTransition::Diagnose
        );
        assert!(
            repair_status(&ctx, &id)
                .unwrap()
                .unwrap()
                .active()
                .unwrap()
                .history
                .coding_blocker()
                .is_some()
        );
    }
}

#[test]
fn diagnosis_is_bounded_non_coding_and_only_recorded_owner_renewal_creates_an_allowance() {
    let _clock = FixedClock::new();
    let (_root, mut ctx, id) = fixture();
    repair_finding(&ctx, &id, report("first", "rollback-test")).unwrap();
    for (attempt, profile) in [
        ("one", "general"),
        ("two", "general"),
        ("three", "deep"),
        ("four", "deep"),
    ] {
        begin(&ctx, &id, attempt, profile, false).unwrap();
    }
    repair_finding(
        &ctx,
        &id,
        report("remaining-after-stronger", "rollback-test"),
    )
    .unwrap();
    ctx.store
        .update_head(&id, |task| {
            task.current_run_mut().unwrap().autopilot = Some(corral_core::AutopilotState {
                stage: corral_core::AutoStage::Escalated,
                reason: Some("PM diagnosis ready".into()),
                sha: None,
                at: ctx.store.now(),
            });
            Ok(())
        })
        .unwrap();
    let peer = ctx
        .store
        .create("Independent", Some("app"), Status::Ready, "")
        .unwrap();
    let preview = ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [id.clone(), peer.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(&preview, "owner", ctx.store.now())
        .unwrap();
    let workset = crate::pm_workset(&ctx).unwrap();
    let action = workset
        .actions
        .iter()
        .find(|a| a.action.kind == crate::PmActionKind::Diagnosis)
        .unwrap();
    let started = crate::pm_act(&ctx, &action.id).unwrap().diagnosis.unwrap();
    assert_eq!(repair_diagnose(&ctx, &id).unwrap(), started);
    let diagnosis = started.active().unwrap().diagnosis.as_ref().unwrap();
    assert_eq!(
        diagnosis.started,
        "2026-11-04T01:00:00Z".parse::<jiff::Timestamp>().unwrap()
    );
    assert_eq!(
        started
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .pm_diagnoses,
        1
    );
    assert!(begin(&ctx, &id, "pm-granted-retry", "deep", false).is_err());
    assert!(repair_diagnosed(&ctx, &id, &diagnosis.id, "", vec![]).is_err());
    let completed = repair_diagnosed(
        &ctx,
        &id,
        &diagnosis.id,
        "Owner proposal: authorize one isolated transaction replan or defer this task",
        vec!["finding:first".into()],
    )
    .unwrap();
    assert!(repair_diagnose(&ctx, &id).is_err());
    let workset = crate::pm_workset(&ctx).unwrap();
    assert!(workset.decisions.iter().any(|d| d.task == id));
    assert!(
        workset
            .actions
            .iter()
            .any(|a| a.action.task.as_ref() == Some(&peer.id))
    );
    assert!(workset.independent_work_continues);
    assert!(matches!(
        completed.transition().unwrap(),
        RepairTransition::Owner { .. }
    ));
    let fake_reset = RepairLedger::new(
        RepairHistory::new(
            RepairPolicy::approved_starter_policy(),
            "general",
            Some("deep".into()),
        )
        .unwrap(),
    );
    assert!(ctx.store.save_repair_ledger(&id, &fake_reset).is_err());
    let renewal = corral_core::RepairRenewal {
        id: "owner-renewal-1".into(),
        previous_digest: completed.digest().unwrap(),
        reason: "Owner authorized a bounded follow-up".into(),
        approved_by: "owner".into(),
        at: ctx.store.now(),
    };
    let renewed = ctx
        .store
        .renew_repair_allowance(
            &id,
            renewal.clone(),
            RepairPolicy::approved_starter_policy(),
            "general",
            Some("deep".into()),
        )
        .unwrap();
    assert_eq!(renewed.allowances.len(), 2);
    assert_eq!(renewed.allowances[0], completed.allowances[0]);
    assert_eq!(
        ctx.store
            .renew_repair_allowance(
                &id,
                renewal,
                RepairPolicy::approved_starter_policy(),
                "general",
                Some("deep".into())
            )
            .unwrap(),
        renewed
    );
    repair_finding(&ctx, &id, report("after-owner-renewal", "rollback-test")).unwrap();
    begin(&ctx, &id, "authorized-new-repair", "general", false).unwrap();
    let reopened = Store::open(ctx.store.root())
        .unwrap()
        .repair_ledger(&id)
        .unwrap()
        .unwrap();
    assert_eq!(
        reopened.allowances[0]
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        4
    );
    assert_eq!(
        reopened
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        1
    );
}

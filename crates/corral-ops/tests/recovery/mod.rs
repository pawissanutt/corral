use super::*;
use std::path::PathBuf;

mod code;
mod landing;
mod planning;
mod rebinding;
mod resources;
mod waiver;

const PLAN: &str = "---\ngoal: recover fixture\nnon_goals: []\ndecisions: []\naffected_paths: [a.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: check, expected: passes}]\nverification: [check]\nowner_decisions: []\n---\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    id: String,
    source: PathBuf,
}
impl Fixture {
    fn new(family: bool) -> Self {
        Self::with_plan(family, PLAN)
    }
    fn with_plan(family: bool, plan: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let family_setting = if family {
            "require_cross_family = true\n"
        } else {
            ""
        };
        let mut store = Store::init_with(tmp.path(), |store| {
            std::fs::write(store.config_path(), format!("[projects.app]\npath = \".\"\n[projects.app.review]\nreviewer = \"gpt-sol\"\nplan_required = true\n{family_setting}[pm]\nagent = \"gpt-sol\"\n[harnesses.fixture]\ncommand = \"true {{model_args}}\"\nmodel_flag = \"--model\"\n[profiles.gpt-sol]\nharness = \"fixture\"\nmodel = \"chatgpt-subscription/gpt-6.1-sol\"\nvendor = \"openai\"\n[profiles.gpt-astra]\nharness = \"fixture\"\nmodel = \"chatgpt-subscription/gpt-6-astra\"\nvendor = \"openai\"\n[profiles.alias]\nharness = \"fixture\"\nmodel = \"gpt-6.1-sol\"\nvendor = \"openai\"\n"))?;
            Ok(())
        }).unwrap();
        store.config.review.plan_accept = Some(Accept::Auto);
        store.save_config().unwrap();
        let task = store
            .create("Recovery fixture", Some("app"), Status::Ready, "Bound spec")
            .unwrap();
        store.update(&task.id, |task| {
            task.plan_runs.push(serde_json::from_value(serde_json::json!({"id":"PL1","profile":"gpt-sol","harness":"fixture","model":"chatgpt-subscription/gpt-6.1-sol","vendor":"openai","spec_revision":crate::plan::plan_spec_revision(task)?,"started":corral_core::clock::now(),"output_dir":"output","rounds":0,"state":"planning"}))?);
            Ok(())
        }).unwrap();
        let source = tmp.path().join("plan.md");
        std::fs::write(&source, plan).unwrap();
        let bin = tmp.path().join("herdr");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
            &bin,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
        ctx.herdr = corral_herdr::Herdr::with_bin(bin, Some("corral-test".into()));
        crate::plan_submit(&ctx, &task.id, &source).unwrap();
        Self {
            _tmp: tmp,
            ctx,
            id: task.id,
            source,
        }
    }
    fn input(&self) -> RecoveryInput {
        let mut exceptions = BTreeSet::from([RecoveryCheck::ConfiguredReviewerBinding]);
        if self
            .ctx
            .store
            .config
            .task_review_policy(&self.ctx.store.get(&self.id).unwrap())
            .cross_family_required()
        {
            exceptions.insert(RecoveryCheck::ReviewerFamilyPolicy);
        }
        RecoveryInput {
            action: RecoveryAction::UseReviewer {
                stage: RecoveryReviewStage::PlanReview,
                reviewer: policy::reviewer(&self.ctx.store.config, "gpt-astra").unwrap(),
            },
            exceptions,
            notes: String::new(),
        }
    }
    fn callback(&self, attempt: &str) -> PlanReview {
        crate::plan_verdict(
            &self.ctx,
            &self.id,
            &crate::PlanVerdict {
                plan: "P1",
                attempt: Some(attempt),
                verdict: Verdict::Approve,
                notes: "Genuine fixture independent review",
            },
        )
        .unwrap()
    }
}

// These are operation tests, not owner-surface evidence. The token is constructed only inside
// this private test module. Subprocesses isolate managed environment and cwd, never the owner store.
fn isolated(name: &str) -> bool {
    if std::env::var("RECOVERY_TEST_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            &format!("recovery::tests::{name}"),
            "--nocapture",
        ])
        .env("RECOVERY_TEST_CHILD", name)
        .current_dir(tmp.path())
        .stdin(std::process::Stdio::null());
    for key in [
        "AI_AGENT",
        "PI_CODING_AGENT",
        "CORRAL_ASSISTANT",
        "CORRAL_PM",
        "CORRAL_TASK",
        "CORRAL_RUN",
        "CORRAL_AGENT_SESSION",
        "CORRAL_AGENT_ROLE",
        "HERDR_PANE_ID",
        "HERDR_SOCKET_PATH",
        "HERDR_SESSION",
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
    ] {
        child.env_remove(key);
    }
    if name.starts_with("landing::") {
        child.env("GIT_TRACE2_EVENT", tmp.path().join("git-events.jsonl"));
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
fn confirmation(preview: &RecoveryPreview) -> OwnerConfirmation {
    OwnerConfirmation {
        preview: preview.clone(),
        authorization: RecoveryAuthorization {
            version: RECOVERY_VERSION,
            preview_digest: preview.digest.clone(),
            reason: "One exact fixture exception".into(),
            attestation: RecoveryAttestation::LocalOwnerAttestation {
                interaction: OwnerInteraction::Cli,
                at: corral_core::clock::now(),
                controlling_terminal: "test-only".into(),
                context: None,
            },
        },
    }
}

#[test]
fn ordinary_same_model_review_is_not_recovery() {
    if isolated("ordinary_same_model_review_is_not_recovery") {
        return;
    }
    let f = Fixture::new(false);
    let review = crate::plan_review(&f.ctx, &f.id, None).unwrap();
    assert_eq!(review.reviewer, "gpt-sol");
    assert_eq!(f.callback(&review.id).verdict, Some(Verdict::Approve));
    crate::plan_verdict(
        &f.ctx,
        &f.id,
        &crate::PlanVerdict {
            plan: "P1",
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Owner accepts ordinary same-model review",
        },
    )
    .unwrap();
    let task = f.ctx.store.get(&f.id).unwrap();
    assert!(crate::approved_plan(&f.ctx, &task).unwrap().is_some());
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
    assert!(!task.dispatch_hold);
}

#[test]
fn family_recovery_binds_launch_callback_owner_acceptance_and_hold() {
    if isolated("family_recovery_binds_launch_callback_owner_acceptance_and_hold") {
        return;
    }
    let f = Fixture::new(true);
    for reviewer in [None, Some("gpt-astra")] {
        let error = crate::plan_review(&f.ctx, &f.id, reviewer).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("matches author model family 'openai'"),
            "{error:#}"
        );
    }
    let task = f.ctx.store.get(&f.id).unwrap();
    assert!(crate::plan::policy_digest(&f.ctx.store.config, &task).is_ok());
    assert!(task.plans[0].reviews.is_empty());
    let config_before = std::fs::read(f.ctx.store.config_path()).unwrap();
    let source_before = std::fs::read(&f.source).unwrap();
    let unrelated = f
        .ctx
        .store
        .create("Unrelated", Some("app"), Status::Backlog, "")
        .unwrap();
    let unrelated_before = f.ctx.store.fixture_task_text(&unrelated.id).unwrap();
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    assert_eq!(preview.request.exceptions, input.exceptions);
    let record = recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    assert_eq!(f.callback("PR1").verdict, Some(Verdict::Approve));
    for _ in 0..2 {
        assert!(crate::plan_review::accept_auto(&f.ctx, &f.id, "P1").is_err());
    }
    assert!(f.ctx.store.get(&f.id).unwrap().plans[0].approval.is_none());
    assert!(
        crate::plan_verdict(
            &f.ctx,
            &f.id,
            &crate::PlanVerdict {
                plan: "P1",
                attempt: None,
                verdict: Verdict::Approve,
                notes: "A caller-selected owner flag is not recovery authority"
            }
        )
        .is_err()
    );
    let notes = "Explicit owner accepts the exact reviewer exception";
    let accept = RecoveryInput {
        action: RecoveryAction::AcceptPlan {
            waive_independent_review: false,
            resolution_notes_sha256: crate::verification::digest(notes.as_bytes()),
        },
        exceptions: input.exceptions.clone(),
        notes: notes.into(),
    };
    let accept_preview = recover_preview(&f.ctx, &f.id, &accept).unwrap();
    assert!(
        accept_preview.remaining_blockers.is_empty(),
        "{:?}",
        accept_preview.remaining_blockers
    );
    let accepted = recover_apply(
        &f.ctx,
        &accept_preview,
        &accept,
        Some(confirmation(&accept_preview)),
    )
    .unwrap();
    assert_eq!(
        state(&accepted).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        accepted.outcomes
    );
    let task = f.ctx.store.get(&f.id).unwrap();
    assert!(task.dispatch_hold && !task.queued);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Approved);
    assert!(accepted.outcomes.last().unwrap().after.is_some());
    assert!(crate::approved_plan(&f.ctx, &task).unwrap().is_some());
    let saved = records(&f.ctx, &f.id).unwrap().pop().unwrap();
    assert_eq!(saved.links.len(), 1);
    let acceptance = saved.links[0].acceptance.as_ref().unwrap();
    assert_eq!(acceptance.acceptance_mode, AcceptanceMode::OwnerRecovery);
    assert!(!acceptance.policy_satisfied);
    assert_eq!(
        std::fs::read(f.ctx.store.config_path()).unwrap(),
        config_before
    );
    assert_eq!(std::fs::read(&f.source).unwrap(), source_before);
    assert_eq!(
        f.ctx.store.fixture_task_text(&unrelated.id).unwrap(),
        unrelated_before
    );
    assert_eq!(
        recover_apply(&f.ctx, &preview, &input, None).unwrap(),
        record
    );
}

#[test]
fn binding_only_preview_and_recovery_model_floor_are_distinct() {
    if isolated("binding_only_preview_and_recovery_model_floor_are_distinct") {
        return;
    }
    let f = Fixture::new(false);
    let mut input = f.input();
    assert!(
        records(&f.ctx, f.id.trim_start_matches("T-"))
            .unwrap()
            .is_empty()
    );
    let before = f.ctx.store.fixture_task_text(&f.id).unwrap();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    assert!(preview.remaining_blockers.is_empty());
    assert_eq!(
        preview.request.exceptions,
        BTreeSet::from([RecoveryCheck::ConfiguredReviewerBinding])
    );
    assert!(
        preview
            .checks
            .iter()
            .any(|c| c.check == RecoveryCheck::ReviewerFamilyPolicy
                && c.disposition == CheckDisposition::Satisfied)
    );
    assert_eq!(
        recover_preview(&f.ctx, &f.id, &input).unwrap().digest,
        preview.digest
    );
    assert_eq!(f.ctx.store.fixture_task_text(&f.id).unwrap(), before);
    for name in ["gpt-sol", "alias"] {
        input.action = RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::PlanReview,
            reviewer: policy::reviewer(&f.ctx.store.config, name).unwrap(),
        };
        assert!(
            recover_preview(&f.ctx, &f.id, &input)
                .unwrap()
                .remaining_blockers
                .iter()
                .any(|b| b.code == "recovery-model-floor")
        );
    }
    // The binding-only variant belongs at the callback contract, not a second F end-to-end
    // journey: seed one exact authorized execution and collect its canonical verdict.
    let record = RecoveryRecord {
        version: RECOVERY_VERSION,
        id: format!("REC-{}", preview.digest),
        preview: preview.clone(),
        authorization: confirmation(&preview).authorization,
        outcomes: vec![
            outcome(RecoveryState::Authorized, "fixture intent", None),
            outcome(RecoveryState::Applying, "fixture separate execution", None),
        ],
        links: vec![],
    };
    f.ctx.store.save_recovery_record(&f.id, &record).unwrap();
    f.ctx
        .store
        .update(&f.id, |task| {
            task.plans[0].reviews.push(PlanReview {
                id: "PR1".into(),
                reviewer: "gpt-astra".into(),
                policy_digest: preview.contract_digest.clone(),
                verdict: None,
                notes: None,
                at: corral_core::clock::now(),
                herdr: Some(HerdrRef {
                    workspace_id: "w1".into(),
                    pane_id: "w1:p1".into(),
                    socket: Some(f.ctx.herdr.socket_path()?),
                }),
                pending_reply: None,
                route: None,
                cancelled: false,
                agent_host: None,
                delivery: Default::default(),
                inbox: Default::default(),
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(f.callback("PR1").verdict, Some(Verdict::Approve));
    assert!(f.ctx.store.get(&f.id).unwrap().reviewer.is_none());
}

#[test]
fn ordinary_explicit_binding_escalates_then_task_selection_succeeds() {
    if isolated("ordinary_explicit_binding_escalates_then_task_selection_succeeds") {
        return;
    }
    let f = Fixture::new(false);
    let explicit = crate::plan_review(&f.ctx, &f.id, Some("gpt-astra")).unwrap();
    assert_eq!(f.callback(&explicit.id).verdict, Some(Verdict::Escalate));
    crate::reviewer_set(&f.ctx, &f.id, Some("gpt-astra")).unwrap();
    let ordinary = crate::plan_review(&f.ctx, &f.id, None).unwrap();
    assert_eq!(f.callback(&ordinary.id).verdict, Some(Verdict::Approve));
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
}

#[test]
fn a_preview_digest_cannot_be_reused_with_a_changed_effect() {
    if isolated("a_preview_digest_cannot_be_reused_with_a_changed_effect") {
        return;
    }
    let f = Fixture::new(true);
    let input = f.input();
    let mut preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    if let RecoveryEffect::ReviewAttempt { attempt, .. } = &mut preview.proposed_effect {
        *attempt = "PR999".into();
    }
    assert!(recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).is_err());
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
}

#[test]
fn apply_rejects_missing_authority_changed_policy_and_changed_bytes() {
    if isolated("apply_rejects_missing_authority_changed_policy_and_changed_bytes") {
        return;
    }
    let mut f = Fixture::new(true);
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    assert!(recover_apply(&f.ctx, &preview, &input, None).is_err());
    let mut blank = confirmation(&preview);
    blank.authorization.reason.clear();
    assert!(recover_apply(&f.ctx, &preview, &input, Some(blank)).is_err());
    let mut unrelated = confirmation(&preview);
    unrelated.preview.request.target.task = "T-999".into();
    assert!(recover_apply(&f.ctx, &preview, &input, Some(unrelated)).is_err());
    let token = confirmation(&preview);
    f.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .require_cross_family = Some(false);
    f.ctx.store.save_config().unwrap();
    assert!(recover_apply(&f.ctx, &preview, &input, Some(token)).is_err());
    f.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .require_cross_family = Some(true);
    f.ctx.store.save_config().unwrap();
    // The draft is disposable; corrupt the exact stored snapshot bound by the preview.
    let task = f.ctx.store.get(&f.id).unwrap();
    let hash = corral_core::blob_ref::BlobHash::parse(&task.plans[0].artifact.sha256).unwrap();
    std::fs::write(
        f.ctx.store.blob_store().unwrap().path_of(&hash),
        format!("{PLAN}Changed bytes"),
    )
    .unwrap();
    assert!(recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).is_err());
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
}

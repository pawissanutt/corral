use super::*;
use clap::Parser;

#[derive(Parser)]
struct Command {
    #[command(flatten)]
    args: RecoverArgs,
}

fn parse(args: &[&str]) -> RecoverArgs {
    Command::try_parse_from(std::iter::once("recover").chain(args.iter().copied()))
        .unwrap()
        .args
}

#[test]
fn closed_action_matrix_maps_only_its_exact_parameters_and_exceptions() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::init(temp.path()).unwrap();
    store.config.harnesses.insert(
        "fixture".into(),
        serde_json::from_value(
            serde_json::json!({"command":"true {model_args}","model_flag":"--model"}),
        )
        .unwrap(),
    );
    store.config.profiles.insert(
        "alternate".into(),
        serde_json::from_value(
            serde_json::json!({"harness":"fixture","model":"reviewer-model","vendor":"openai"}),
        )
        .unwrap(),
    );
    store.config.resources.max_load_per_core = f64::MAX;
    store.config.resources.min_mem_available = "1B".into();
    store.config.resources.min_disk_free = "1B".into();
    store.save_config().unwrap();
    let ctx = Ctx::from_store(store);
    let reviewer = parse(&[
        "T-001",
        "--action",
        "use-reviewer",
        "--stage",
        "plan-review",
        "--reviewer",
        "alternate",
        "--waive",
        "configured-reviewer-binding",
    ])
    .input(&ctx)
    .unwrap();
    assert!(matches!(
        reviewer.action,
        RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::PlanReview,
            ..
        }
    ));
    assert_eq!(
        reviewer.exceptions,
        BTreeSet::from([RecoveryCheck::ConfiguredReviewerBinding])
    );
    let notes = temp.path().join("notes.md");
    std::fs::write(&notes, "Exact resolutions").unwrap();
    let plan = parse(&[
        "T-001",
        "--action",
        "accept-plan",
        "--notes-file",
        notes.to_str().unwrap(),
        "--waive",
        "independent-plan-review",
    ])
    .input(&ctx)
    .unwrap();
    assert_eq!(
        plan.action,
        RecoveryAction::AcceptPlan {
            waive_independent_review: true,
            resolution_notes_sha256: corral_core::agent::paths::sha256_hex(b"Exact resolutions")
        }
    );
    assert_eq!(plan.notes, "Exact resolutions");
    let code = parse(&[
        "T-001",
        "--action",
        "accept-code",
        "--merge",
        "ff",
        "--waive",
        "gate-outcome,review-outcome",
    ])
    .input(&ctx)
    .unwrap();
    assert_eq!(
        code.action,
        RecoveryAction::AcceptCode {
            merge: RecoveryMerge::Ff
        }
    );
    assert_eq!(
        code.exceptions,
        BTreeSet::from([RecoveryCheck::GateOutcome, RecoveryCheck::ReviewOutcome])
    );
    let force = parse(&[
        "T-001",
        "--action",
        "force-admission",
        "--stage",
        "gate",
        "--waive",
        "resource-pressure",
    ])
    .input(&ctx)
    .unwrap();
    assert!(matches!(
        force.action,
        RecoveryAction::ForceAdmission {
            stage: RecoveryAdmissionStage::Gate,
            ..
        }
    ));
    assert_eq!(
        force.exceptions,
        BTreeSet::from([RecoveryCheck::ResourcePressure])
    );
    for args in [
        vec![
            "T-001",
            "--action",
            "accept-code",
            "--merge",
            "ff",
            "--reviewer",
            "alternate",
        ],
        vec![
            "T-001",
            "--action",
            "use-reviewer",
            "--stage",
            "dispatch",
            "--reviewer",
            "alternate",
        ],
        vec!["T-001", "--action", "accept-plan"],
    ] {
        assert!(parse(&args).input(&ctx).is_err());
    }
    for args in [
        vec!["T-001", "--action", "force"],
        vec!["T-001", "--action", "accept-code", "--yes"],
        vec!["T-001", "--action", "accept-plan", "--waive", "all"],
        vec![
            "T-001",
            "--history",
            "--apply",
            "digest",
            "--reason",
            "reason",
        ],
        vec!["T-001", "--action", "accept-code", "--apply", "digest"],
    ] {
        assert!(Command::try_parse_from(std::iter::once("recover").chain(args)).is_err());
    }
}

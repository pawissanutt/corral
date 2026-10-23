use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;
use sha2::{Digest, Sha256};

use crate::*;

// Canonical bytes captured on main a26c7b76, before any tier field existed.
const IDENTITY: &str = r#"{"version":2,"task":"T-900","run":"R1","attempt":"a1","head":"h1","tree":"t1","integration_base":"b1","dispatch_base":"d1","spec_digest":"s1","policy_digest":"p1","inputs_digest":"i1","environment":[["PATH","/bin"],["RUSTC",null]]}"#;
const POLICY_DEFAULT: &str = r#"{"plan_required":false,"plan_accept":"owner","gate":[],"env":{},"size_limits":{"rs":800},"reviewer":"None","require_cross_family":false,"accept":"owner","max_rounds":2,"merge":"rebase","remove_worktree":true,"owner_paths":[]}"#;
const POLICY_CONFIGURED: &str = r#"{"plan_required":false,"plan_accept":"owner","gate":["scripts/gate.sh"],"env":{"A":"1"},"size_limits":{"rs":800},"reviewer":"Pm","require_cross_family":false,"accept":"auto","max_rounds":2,"merge":"rebase","remove_worktree":true,"owner_paths":[],"risk_paths":["src/**"]}"#;
const TRACKER: &str = "---\nschema_version: 2\ntask:\n  id: T-900\n  title: Tracker\n  status: backlog\n  priority: P2\n  tracker: true\n  approved_scope:\n    goal: g\n    acceptance:\n    - scenario: s\n      expected: e\n    delivery_target:\n      kind: integration\n      branch: plan/T-900\n  children:\n  - T-901\n  created: 2026-10-23T00:00:00Z\n  updated: 2026-10-23T00:00:00Z\n---\nBody\n";
const TRACKER_REVISION: &str = "489eef2ba65f627e3d85d5271801a98b25fe8ed5584720002e54b4615884117b";
const MEMBER: &str = "---\nschema_version: 2\ntask:\n  id: T-901\n  title: Member\n  status: backlog\n  priority: P2\n  plan: T-899@P1\n  parent: T-900\n  created: 2026-10-23T00:00:00Z\n  updated: 2026-10-23T00:00:00Z\n  runs:\n  - delivery: legacy\n    inbox:\n      next_seq: 1\n      next_envelope: 1\n      open_envelope: null\n      effects_applied: []\n      binding: null\n      observe_cursor:\n        offset: 0\n        last_complete_line: 0\n        coverage: complete\n    completion: {}\n    id: R1\n    agent: claude\n    mode: interactive\n    worktree: /w\n    branch: corral/T-901\n    base: main\n    base_sha: b0\n    started: 2026-10-23T00:00:00Z\n    seen_working: false\n---\nBody\n";
const PLAN_JSON: &str = r#"{"delivery_target":{"kind":"integration","branch":"plan/T-900"},"goal":"g","non_goals":[],"decisions":[],"affected_paths":["src"],"contracts":[],"dependencies":[{"task":"a","write_scope":["src/a.rs"],"depends_on":[]},{"task":"b","write_scope":["src/b.rs"],"depends_on":["a"],"acceptance":[{"scenario":"s","expected":"e"}]}],"acceptance":[{"scenario":"s","expected":"e"}],"verification":["cargo test"],"owner_decisions":[]}"#;
const SCOPE_DIGESTS: [(&str, &str); 2] = [
    (
        "a",
        "e66657ef70d730d3bd12f6acda64fda7c00ffc9b4109f0663543594a9b4be0cd",
    ),
    (
        "b",
        "4f611c012127728288718e5fa94ad96576388e89fed33b13ce229f0b550618aa",
    ),
];

fn gate_json() -> String {
    format!(
        r#"{{"identity":{IDENTITY},"commands":[{{"command":"scripts/gate.sh","exit_code":0,"log_start":0,"log_end":10}}],"log_sha256":"l1","result":"pass","at":"2026-10-23T00:00:00Z","sha":"h1"}}"#
    )
}

fn completion_json() -> String {
    format!(
        r#"{{"required_checks":["scripts/gate.sh"],"manifest":{{"version":1,"spec_digest":"s1","reviewed_by":"owner","audited":true,"require_gate":true,"criteria":[{{"id":"C1","required":true,"live":false,"scenario":"s","expected":"e"}}],"owner_decisions":[]}},"evidence":[{{"source":"worker_assertion","observation":{{"version":1,"key":"k1","criterion":"C1","run":"R1","attempt":"a1","status":"pass","action":"a","environment":"env","expected":"e","observed":"o","artifacts":[],"captured":"2026-10-23T00:00:01Z","producer":"p","live":false,"cleanup":null}},"identity":{IDENTITY},"manifest_digest":"m1","artifact_hashes":[]}}]}}"#
    )
}

fn run_json() -> String {
    format!(
        r#"{{"delivery":"legacy","inbox":{{"next_seq":1,"next_envelope":1,"open_envelope":null,"effects_applied":[],"binding":null,"observe_cursor":{{"offset":0,"last_complete_line":0,"coverage":"complete"}}}},"completion":{{}},"id":"R1","agent":"claude","mode":"interactive","worktree":"/w","branch":"corral/T-900","base":"main","base_sha":"b0","started":"2026-10-23T00:00:00Z","seen_working":false,"gate":{}}}"#,
        gate_json()
    )
}

fn plan_text(target: &str, scopes: &str) -> String {
    format!(
        "---\n{target}goal: g\nnon_goals: []\ndecisions: []\naffected_paths: [src]\ncontracts: []\ndependencies:\n{scopes}acceptance:\n- scenario: s\n  expected: e\nverification: [cargo test]\nowner_decisions: []\n---\n"
    )
}

const INTEGRATION: &str = "delivery_target:\n  kind: integration\n  branch: plan/T-900\n";
const TWO_SCOPES: &str = "- task: a\n  write_scope: [src/a.rs]\n  depends_on: []\n- task: b\n  write_scope: [src/b.rs]\n  depends_on: [a]\n  acceptance:\n  - scenario: s\n    expected: e\n";

fn gated_plan(target: &str, gate: &str) -> String {
    let scopes = format!(
        "- task: a\n  write_scope: [src/a.rs]\n  depends_on: []\n  gate:\n{gate}- task: b\n  write_scope: [src/b.rs]\n  depends_on: [a]\n"
    );
    plan_text(target, &scopes)
}

fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(json: &str) -> T {
    let value: T = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&value).unwrap(), json);
    value
}

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(1_792_713_600 + second).unwrap()
}

fn binding(tier: &str, source: &str) -> GateTierBinding {
    GateTierBinding {
        tier: tier.into(),
        selection_sha256: format!("sel-{tier}"),
        source: source.into(),
    }
}

#[test]
fn untiered_records_serialize_to_todays_bytes() {
    let identity: CandidateIdentity = round_trip(IDENTITY);
    assert_eq!(identity.gate_tier, None);
    let gate: GateRecord = round_trip(&gate_json());
    assert_eq!(gate.tier, None);
    let completion: CompletionState = round_trip(&completion_json());
    assert_eq!(completion.evidence_carry, None);
    let run: Run = round_trip(&run_json());
    assert_eq!(run.gate_rules, GateRules::Legacy);
    assert_eq!(run.promotion_origin, None);
    let member = parse_task(MEMBER, Path::new("/t/T-901.md")).unwrap();
    assert_eq!(member.runs[0].gate_rules, GateRules::Legacy);
    assert_eq!(render_task(&member), MEMBER);
}

#[test]
fn untiered_review_policies_serialize_to_todays_bytes() {
    let mut config = Config::default_for(Path::new("."));
    let policy = config.review_policy(None, None);
    assert_eq!(serde_json::to_string(&policy).unwrap(), POLICY_DEFAULT);
    config.review = toml::from_str(
        "gate = ['scripts/gate.sh']\nenv = { A = '1' }\nreviewer = 'pm'\nrisk_paths = ['src/**']\naccept = 'auto'",
    )
    .unwrap();
    let policy = config.review_policy(None, None);
    assert_eq!(serde_json::to_string(&policy).unwrap(), POLICY_CONFIGURED);
}

#[test]
fn plans_without_gate_parse_and_digest_as_before() {
    let plan = PlanDocument::parse(&plan_text(INTEGRATION, TWO_SCOPES)).unwrap();
    assert_eq!(serde_json::to_string(&plan).unwrap(), PLAN_JSON);
    for (scope, (label, digest)) in plan.dependencies.iter().zip(SCOPE_DIGESTS) {
        assert_eq!((scope.task.as_str(), &scope.gate), (label, &None));
        let bytes = serde_json::to_vec(scope).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), digest);
    }
}

#[test]
fn member_gates_persist_outside_the_operating_contract_revision() {
    let tracker = parse_task(TRACKER, Path::new("/t/T-900.md")).unwrap();
    assert!(tracker.member_gates.is_empty());
    assert_eq!(render_task(&tracker), TRACKER);
    assert_eq!(
        operating_contract_revision(&tracker).unwrap(),
        TRACKER_REVISION
    );
    let first = MemberGateRecord {
        revision: 1,
        gate: ScopeGate {
            tier: "quick".into(),
            tests: vec!["package(corral-ops)".into()],
            expected_failures: Vec::new(),
        },
        reason: "tighten".into(),
        by: "owner".into(),
        at: at(0),
        history: Vec::new(),
    };
    let mut amended = tracker.clone();
    amended.member_gates.insert(
        "T-901".into(),
        MemberGateRecord {
            revision: 2,
            gate: ScopeGate {
                tier: FULL_TIER.into(),
                tests: Vec::new(),
                expected_failures: Vec::new(),
            },
            reason: "core contract".into(),
            by: "owner".into(),
            at: at(1),
            history: vec![first],
        },
    );
    let text = render_task(&amended);
    assert_ne!(text, TRACKER);
    let reread = parse_task(&text, Path::new("/t/T-900.md")).unwrap();
    assert_eq!(reread.member_gates, amended.member_gates);
    assert_eq!(
        operating_contract_revision(&amended).unwrap(),
        TRACKER_REVISION
    );
}

#[test]
fn tier_fields_round_trip_and_enter_identity_differences() {
    let full: CandidateIdentity = serde_json::from_str(IDENTITY).unwrap();
    let quick = CandidateIdentity {
        gate_tier: Some(Box::new(binding("quick", "policy:review_rounds"))),
        ..full.clone()
    };
    let json = serde_json::to_string(&quick).unwrap();
    assert!(json.ends_with(
        r#","gate_tier":{"tier":"quick","selection_sha256":"sel-quick","source":"policy:review_rounds"}}"#
    ));
    assert_eq!(round_trip::<CandidateIdentity>(&json), quick);
    let changes = quick.differences(&full);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (
            changes[0].field.as_str(),
            changes[0].expected.as_str(),
            changes[0].actual.as_str()
        ),
        (
            "gate_tier",
            "quick from policy:review_rounds (sel-quick)",
            FULL_TIER
        )
    );

    let promotion = GatePromotion {
        identity: quick.clone(),
        gate_at: at(2),
    };
    let mut run: Run = serde_json::from_str(&run_json()).unwrap();
    run.gate_rules = GateRules::Tiered;
    run.promotion_origin = Some(Box::new(promotion.clone()));
    let gate = run.gate.as_mut().unwrap();
    gate.tier = Some(Box::new(GateTierReceipt {
        tier: FULL_TIER.into(),
        context_sha256: "ctx".into(),
        carried: Some(GateCarry {
            from_head: "h0".into(),
            from_tree: "t0".into(),
            from_integration_base: "b0".into(),
            old_patch_sha256: "p0".into(),
            new_patch_sha256: "p0".into(),
            at: at(3),
        }),
        promoted_from: Some(promotion),
    }));
    run.completion.evidence_carry = Some(Box::new(EvidenceCarry {
        version: 1,
        method: CarryMethod::CleanReplayTierPromotion,
        from: quick,
        window_start: at(2),
        to: full,
        records_sha256: "records".into(),
        at: at(4),
    }));
    let json = serde_json::to_string(&run).unwrap();
    assert!(json.contains(r#""gate_rules":"tiered""#));
    assert!(json.contains(r#""method":"clean_replay+tier_promotion""#));
    assert_eq!(serde_json::from_str::<Run>(&json).unwrap(), run);
    for (method, name) in [
        (CarryMethod::TierPromotion, "tier_promotion"),
        (CarryMethod::CleanReplay, "clean_replay"),
    ] {
        assert_eq!(serde_json::to_string(&method).unwrap(), format!("{name:?}"));
    }
}

const TIERED: &str = r#"
[review]
gate = ["scripts/gate.sh"]
gate_tiers = { quick = ["scripts/gate.sh --tier quick"], lint = ["cargo clippy"] }
tier_policy = { plan_member = "quick" }
[projects.app]
path = "."
[projects.app.review]
gate_tiers = { quick = ["just quick"] }
tier_policy = { review_rounds = "lint" }
"#;

#[test]
fn tiers_merge_by_key_and_tier_policy_by_field() {
    let config: Config = toml::from_str(TIERED).unwrap();
    assert_eq!(config.review_problems(), Vec::<String>::new());
    let project = config.review_policy(Some("app"), None);
    assert_eq!(
        project.gate_tiers,
        BTreeMap::from([
            ("lint".to_string(), vec!["cargo clippy".to_string()]),
            ("quick".to_string(), vec!["just quick".to_string()]),
        ])
    );
    assert_eq!(
        project.tier_policy,
        TierPolicy {
            plan_member: Some("quick".into()),
            review_rounds: Some("lint".into()),
        }
    );
    let global = config.review_policy(None, None);
    assert_eq!(
        global.gate_tiers["quick"],
        vec!["scripts/gate.sh --tier quick".to_string()]
    );
    assert_eq!(global.tier_policy.review_rounds, None);
    let json = serde_json::to_string(&global).unwrap();
    assert!(json.ends_with(
        r#""gate_tiers":{"lint":["cargo clippy"],"quick":["scripts/gate.sh --tier quick"]},"tier_policy":{"plan_member":"quick"}}"#
    ));
    let sources = |project| {
        let s = config.review_policy_sources(project, None);
        (
            s.gate_tiers,
            s.tier_policy_plan_member,
            s.tier_policy_review_rounds,
        )
    };
    use PolicySource::{Default, Global, Project};
    assert_eq!(sources(Some("app")), (Project, Global, Project));
    assert_eq!(sources(None), (Global, Global, Default));
}

#[test]
fn tier_validation_names_the_layer() {
    let config: Config = toml::from_str(
        r#"
[review]
gate_tiers = { full = ["x"], empty = [], Bad_Name = ["x"], blank = [" "] }
tier_policy = { plan_member = "missing", review_rounds = "local" }
plan_checkpoint_every = 0
[projects.app]
path = "."
[projects.app.review]
gate_tiers = { local = ["x"] }
tier_policy = { review_rounds = "nope", plan_member = "local" }
plan_checkpoint_every = 0
[projects.ok]
path = "."
[projects.ok.review]
tier_policy = { review_rounds = "blank", plan_member = "full" }
"#,
    )
    .unwrap();
    let problems = config.review_problems();
    for expected in [
        "[review]: gate_tiers.full is reserved for gate",
        "[review]: gate_tiers.empty has no commands",
        "[review]: gate_tiers: tier name \"Bad_Name\" must match [a-z][a-z0-9-]*",
        "[review]: gate_tiers.blank has an empty command",
        "[review]: tier_policy.plan_member names unknown tier \"missing\" (defined: full, Bad_Name, blank, empty)",
        "[review]: tier_policy.review_rounds names unknown tier \"local\" (defined: full, Bad_Name, blank, empty)",
        "[review]: plan_checkpoint_every must be at least 1",
        "[projects.app.review]: tier_policy.review_rounds names unknown tier \"nope\" (defined: full, Bad_Name, blank, empty, local)",
        "[projects.app.review]: plan_checkpoint_every must be at least 1",
    ] {
        assert!(
            problems.iter().any(|p| p == expected),
            "missing {expected:?} in {problems:#?}"
        );
    }
    assert_eq!(problems.len(), 9, "{problems:#?}");
    assert!(config.validate_review().is_err());
    let negative = toml::from_str::<Config>("[review]\nplan_checkpoint_every = -1").unwrap_err();
    assert!(
        negative.to_string().contains("plan_checkpoint_every"),
        "{negative}"
    );
}

#[test]
fn plan_checkpoint_every_is_operational() {
    let base: Config = toml::from_str(TIERED).unwrap();
    let mut changed = base.clone();
    changed.review.plan_checkpoint_every = Some(5);
    changed
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .plan_checkpoint_every = Some(2);
    assert_eq!(base.plan_checkpoint_every(Some("app")), None);
    assert_eq!(changed.plan_checkpoint_every(None), Some(5));
    assert_eq!(changed.plan_checkpoint_every(Some("app")), Some(2));
    assert_eq!(changed.plan_checkpoint_every(Some("other")), Some(5));
    for project in [None, Some("app")] {
        let (before, after) = (
            base.review_policy(project, None),
            changed.review_policy(project, None),
        );
        assert_eq!(
            serde_json::to_vec(&before).unwrap(),
            serde_json::to_vec(&after).unwrap()
        );
        assert_eq!(
            reviewer_policy_digest(&before).unwrap(),
            reviewer_policy_digest(&after).unwrap()
        );
        assert_eq!(
            changed
                .review_policy_sources(project, None)
                .plan_checkpoint_every,
            if project.is_some() {
                PolicySource::Project
            } else {
                PolicySource::Global
            }
        );
    }
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    assert_eq!(
        store.policy_revision(&base).unwrap(),
        store.policy_revision(&changed).unwrap()
    );
    let mut tiers = base.clone();
    tiers.review.gate_tiers = None;
    assert_ne!(
        store.policy_revision(&base).unwrap(),
        store.policy_revision(&tiers).unwrap()
    );
    let only: Config = toml::from_str("[review]\nplan_checkpoint_every = 3").unwrap();
    assert!(!only.has_review_policy(None, None));
}

#[test]
fn scope_gates_parse_only_in_integration_plans_with_two_scopes() {
    let quick = "    tier: quick\n    tests: ['package(corral-ops)']\n    expected_failures:\n    - test: corral-ops::gate_ops tests::quick_gate_runs\n      reason: restored by tier-selection-identity\n";
    let plan = PlanDocument::parse(&gated_plan(INTEGRATION, quick)).unwrap();
    let gate = plan.dependencies[0].gate.as_ref().unwrap();
    assert_eq!(
        (gate.tier.as_str(), gate.tests.as_slice()),
        ("quick", ["package(corral-ops)".to_string()].as_slice())
    );
    assert_eq!(
        gate.expected_failures,
        vec![ExpectedFailure {
            test: "corral-ops::gate_ops tests::quick_gate_runs".into(),
            reason: "restored by tier-selection-identity".into(),
        }]
    );
    assert!(PlanDocument::parse(&gated_plan(INTEGRATION, "    tier: full\n")).is_ok());

    let single = plan_text(
        INTEGRATION,
        "- task: a\n  write_scope: [src/a.rs]\n  depends_on: []\n  gate:\n    tier: quick\n",
    );
    let failure = |yaml_id: &str| {
        format!("    tier: quick\n    expected_failures:\n    - test: {yaml_id}\n      reason: r\n")
    };
    for (plan, expected) in [
        (
            gated_plan("", quick),
            "gate is valid only in an integration-target plan with two or more scopes",
        ),
        (
            gated_plan("delivery_target:\n  kind: main\n", quick),
            "gate is valid only in an integration-target plan with two or more scopes",
        ),
        (
            single,
            "gate is valid only in an integration-target plan with two or more scopes",
        ),
        (
            gated_plan(INTEGRATION, "    tier: full\n    tests: [x]\n"),
            "tier full takes no tests or expected failures",
        ),
        (
            gated_plan(
                INTEGRATION,
                "    tier: full\n    expected_failures:\n    - test: a b\n      reason: r\n",
            ),
            "tier full takes no tests or expected failures",
        ),
        (
            gated_plan(INTEGRATION, "    tier: quick\n    tests: ['  ']\n"),
            "tests must not contain blank entries",
        ),
        (
            gated_plan(INTEGRATION, "    tier: quick\n    tests: [\"a\\u0007\"]\n"),
            "contains a control character",
        ),
        (
            gated_plan(
                INTEGRATION,
                "    tier: quick\n    expected_failures:\n    - test: a b\n",
            ),
            "missing field `reason`",
        ),
        (
            gated_plan(
                INTEGRATION,
                "    tier: quick\n    expected_failures:\n    - test: a b\n      reason: ' '\n",
            ),
            "needs a reason",
        ),
        (
            gated_plan(INTEGRATION, &failure("'corral-ops::gate_ops'")),
            "must have the form",
        ),
        (
            gated_plan(INTEGRATION, &failure("' tests::a'")),
            "must have the form",
        ),
        (
            gated_plan(INTEGRATION, &failure("'bin tests::a '")),
            "must have the form",
        ),
        (
            gated_plan(INTEGRATION, &failure("'bin '")),
            "must have the form",
        ),
        (
            gated_plan(INTEGRATION, &failure(r#""bin tests::a\e""#)),
            "contains a control character",
        ),
        (
            gated_plan(INTEGRATION, "    tier: Quick\n"),
            "must match [a-z][a-z0-9-]*",
        ),
        (
            gated_plan(INTEGRATION, "    tier: quick\n    scope: [x]\n"),
            "unknown field `scope`",
        ),
    ] {
        let error = format!("{:#}", PlanDocument::parse(&plan).unwrap_err());
        assert!(error.contains(expected), "{expected:?} not in {error:?}");
    }
}

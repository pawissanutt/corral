use std::path::Path;
use std::process::{Command, Output, Stdio};

fn corral(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    let out = command
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn pm_next_and_brief_are_single_json_readbacks_without_writes() {
    // Given an isolated board, with no herdr interaction or network.
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("app/.git")).unwrap();
    corral(temp.path(), &["init"]);
    let mut store = corral_core::Store::open(temp.path()).unwrap();
    store.config.resources.max_load_per_core = f64::MAX;
    store.config.resources.min_mem_available = "1B".into();
    store.config.resources.min_disk_free = "1B".into();
    store.save_config().unwrap();
    corral(
        temp.path(),
        &[
            "new",
            "An unpromoted idea",
            "--idea",
            "--project",
            "app",
            "--body",
            "## Acceptance\nThe CLI emits a read-only promotion card.",
        ],
    );
    let before = std::fs::read(temp.path().join(".corral/events.jsonl")).unwrap();
    // When querying twice and requesting a compact brief.
    let first = corral(temp.path(), &["pm", "next", "--json"]);
    let second = corral(temp.path(), &["pm", "next", "--json"]);
    let brief = corral(temp.path(), &["pm", "brief", "--json"]);
    // Then each next is exactly one identical object, and the brief embeds that object.
    let a: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&brief.stdout).unwrap();
    assert!(a.is_object());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(a["kind"], "promote");
    assert_eq!(a["requires_owner"], true);
    assert_eq!(b["next"], a);
    assert_eq!(a["workset"]["mode"], "assist");
    assert!(a["workset"]["actions"].as_array().unwrap().is_empty());
    assert_ne!(a["workset"]["status"], "complete");
    assert_eq!(b["operating"], a["workset"]);
    assert_eq!(
        before,
        std::fs::read(temp.path().join(".corral/events.jsonl")).unwrap()
    );
}

#[test]
fn pm_presentation_and_owner_answer_use_separate_persisted_operations() {
    // Given a workspace decision without a fabricated worker run.
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("app/.git")).unwrap();
    corral(temp.path(), &["init"]);
    let store = corral_core::Store::open(temp.path()).unwrap();
    let task = store
        .create("Held", Some("app"), corral_core::Status::Ready, "Finite")
        .unwrap();
    store
        .update(&task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let next: serde_json::Value =
        serde_json::from_slice(&corral(temp.path(), &["pm", "next", "--json"]).stdout).unwrap();
    let decision = next["workset"]["decisions"][0]["id"].as_str().unwrap();
    corral(
        temp.path(),
        &["pm", "present", decision, "--delivery", "fixture-receipt"],
    );
    let uncertain: serde_json::Value =
        serde_json::from_slice(&corral(temp.path(), &["pm", "brief", "--json"]).stdout).unwrap();
    assert_eq!(
        uncertain["operating"]["decisions"][0]["presentation"]["confirmed"],
        false
    );
    // When uncertainty becomes confirmed, redelivery is idempotent, then the owner answers.
    for _ in 0..2 {
        corral(
            temp.path(),
            &[
                "pm",
                "present",
                decision,
                "--delivery",
                "fixture-receipt",
                "--confirmed",
            ],
        );
    }
    let pending: serde_json::Value =
        serde_json::from_slice(&corral(temp.path(), &["pm", "brief", "--json"]).stdout).unwrap();
    assert_eq!(pending["operating"]["status"], "needs_your_decision");
    assert_eq!(
        pending["operating"]["decisions"][0]["presentation"]["confirmed"],
        true
    );
    corral(
        temp.path(),
        &[
            "pm",
            "resolve",
            decision,
            "--answer",
            "Leave this task held",
        ],
    );
    // Then resolving the workspace question neither grants scope nor clears the task hold.
    let state: serde_json::Value = serde_json::from_slice(
        &corral(temp.path(), &["pm", "act", "stale-action", "--json"]).stdout,
    )
    .unwrap();
    assert_eq!(state["outcome"], "stale");
    assert!(state["workset"]["actions"].as_array().unwrap().is_empty());
    assert!(store.get(&task.id).unwrap().dispatch_hold);
    assert!(store.get(&task.id).unwrap().runs.is_empty());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn concurrent_presentation_preserves_an_owner_answer_across_reopening() {
    // Given a presenter fenced before its board lock, when another CLI answers,
    // then the exact answer survives presentation and the question stays resolved.
    let output = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pm_support/presentation_race.py"))
        .arg(env!("CARGO_BIN_EXE_corral"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shipped_replay_scores_seven_criteria_without_an_initialized_board() {
    // Given the authentic frozen history, outside any corral store.
    let temp = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../corral-ops/tests/fixtures/pm-2026-10-15.events.jsonl");
    // When the actual CLI replays it with its default digest-bound cases.
    let out = corral(
        temp.path(),
        &[
            "pm",
            "bench",
            "--events",
            fixture.to_str().unwrap(),
            "--json",
        ],
    );
    // Then the baseline has a real score covering all seven criteria, without creating a store.
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["covered_criteria"].as_array().unwrap().len(), 7);
    assert!(report["missing_criteria"].as_array().unwrap().is_empty());
    let baseline = &report["models"][0];
    assert_eq!(baseline["policy_violations"], 0);
    assert!(baseline["passed"].as_u64().unwrap() >= 7);
    assert!(!temp.path().join(".corral").exists());
}

#[test]
fn pm_brief_ranks_lessons_by_pins_decayed_confidence_and_id() {
    for (pinned, contradicted, old_confidence, old_age_hours, expected) in [
        (false, false, 1.0, 8760, "recent"),
        (true, false, 0.1, 8760, "old"),
        (true, true, 1.0, 8760, "recent"),
        (false, false, 0.6, 0, "old"),
    ] {
        // Given a top-one project brief with old and recent advice.
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app/.git")).unwrap();
        corral(temp.path(), &["init"]);
        let mut store = corral_core::Store::open(temp.path()).unwrap();
        store.config.lessons.top = 1;
        store.save_config().unwrap();
        let now = jiff::Timestamp::now();
        let recent = corral_core::Lesson {
            id: "recent".into(),
            rule: "Recent rule".into(),
            why: "Recent evidence".into(),
            evidence: vec![corral_core::LessonEvidence {
                task: "T-001".into(),
                ts: now,
                source: "event:verdict".into(),
            }],
            scope: corral_core::LessonScope::default(),
            confidence: 0.6,
            pinned: false,
            contradicted: false,
            last_seen: now,
        };
        let mut old = recent.clone();
        old.id = "old".into();
        old.confidence = old_confidence;
        old.pinned = pinned;
        old.contradicted = contradicted;
        old.last_seen = now
            .checked_sub(jiff::SignedDuration::from_hours(old_age_hours))
            .unwrap();
        store.save_lessons("app", &[recent, old]).unwrap();
        // When the actual PM brief selects and truncates project lessons.
        let out = corral(temp.path(), &["pm", "brief", "--json"]);
        let brief: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        // Then decay, pins, contradiction exclusion and ID ties precede top-N.
        assert_eq!(brief["lessons"]["app"].as_array().unwrap().len(), 1);
        assert_eq!(brief["lessons"]["app"][0]["id"], expected);
    }
}

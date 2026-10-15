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
    corral(temp.path(), &["init"]);
    corral(temp.path(), &["new", "An unpromoted idea", "--idea"]);
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
    assert_eq!(a["kind"], "idea_promotion");
    assert_eq!(a["requires_owner"], true);
    assert_eq!(b["next"], a);
    assert_eq!(
        before,
        std::fs::read(temp.path().join(".corral/events.jsonl")).unwrap()
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

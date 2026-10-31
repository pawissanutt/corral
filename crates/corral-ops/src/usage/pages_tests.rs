use super::*;

fn fixture(harness: &str) -> (tempfile::TempDir, Store, Run, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let path = temp.path().join("transcript.jsonl");
    let run = serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":harness, "harness":harness, "worktree":"/fixture/project",
        "branch":"test", "base":"main", "base_sha":"abc",
        "started":"2026-09-01T00:00:00Z", "ended":"2026-11-01T00:00:00Z",
        "session":{"harness":harness,"directory":temp.path(),"transcript":path,"policy":"reuse","action":"new"}
    }))
    .unwrap();
    (temp, store, run, path)
}

#[test]
fn legacy_pages_filter_cwd_and_window_and_page_records() {
    for harness in ["omo", "claude"] {
        let (_temp, store, run, path) = fixture(harness);
        fs::write(&path, concat!(
            "{\"type\":\"session\",\"cwd\":\"/other\",\"timestamp\":\"2026-10-01T00:00:00Z\"}\n",
            "{\"type\":\"message\",\"text\":\"foreign\",\"timestamp\":\"2026-10-01T00:00:00Z\"}\n",
            "{\"type\":\"session\",\"cwd\":\"/fixture/project\",\"timestamp\":\"2026-10-01T00:00:00Z\"}\n",
            "{\"type\":\"message\",\"text\":\"wanted\",\"timestamp\":\"2026-10-01T00:00:00Z\"}\n",
            "{\"type\":\"message\",\"text\":\"too late\",\"timestamp\":\"2026-12-01T00:00:00Z\"}\n"
        )).unwrap();
        let text = page(&run, &store, 2, 2).unwrap();
        assert!(text.contains("wanted"), "{harness}: {text}");
        assert!(!text.contains("foreign") && !text.contains("too late"));
        assert!(page(&run, &store, 3, 200).unwrap().is_empty());
        fs::remove_file(&path).unwrap();
        let error = format!("{:#}", page(&run, &store, 1, 200).unwrap_err());
        assert!(error.contains("transcript_unavailable") && error.contains(path.to_str().unwrap()));
    }
}

#[test]
fn output_and_input_are_bounded() {
    let (_temp, store, run, path) = fixture("omo");
    let line = serde_json::json!({"type":"message", "cwd":"/fixture/project", "timestamp":"2026-10-01T00:00:00Z", "text":"x".repeat(7000)}).to_string() + "\n";
    fs::write(&path, line.repeat(10)).unwrap();
    let text = page(&run, &store, 1, 200).unwrap();
    assert!(text.len() <= PAGE_BYTES);
    assert!(text.contains("resume with --lines 4-203"));
    let long = serde_json::json!({"type":"message", "cwd":"/fixture/project", "timestamp":"2026-10-01T00:00:00Z", "text":"界".repeat(PAGE_BYTES)}).to_string() + "\n";
    fs::write(&path, long + &line).unwrap();
    let text = page(&run, &store, 1, 200).unwrap();
    assert!(text.len() <= PAGE_BYTES && text.contains("record truncated"));
    assert!(text.contains("resume with --lines 2-201"));
    assert!(
        page(&run, &store, 2, 200)
            .unwrap()
            .contains(&"x".repeat(7000))
    );
    fs::write(&path, "x".repeat(RECORD_BYTES * 2)).unwrap();
    assert!(
        page(&run, &store, 1, 200)
            .unwrap_err()
            .to_string()
            .contains("record limit")
    );
    assert!(
        transcript_page(&store, "missing", "R1", None, 0, 200)
            .unwrap_err()
            .to_string()
            .contains("invalid transcript range")
    );
}

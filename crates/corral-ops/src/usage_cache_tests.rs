use super::*;
use std::io::Write;

// Cache counter fixtures must not overlap a test that advances board generations.
static CACHE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

const HEADER: &str = "{\"type\":\"session\",\"cwd\":\"/fixture/project\"}\n";

fn response(id: &str, input: u64, output: u64) -> String {
    format!(
        "{{\"type\":\"message\",\"id\":\"{id}\",\"timestamp\":\"2026-10-01T00:00:01Z\",\
         \"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":{input},\"output\":{output}}}}}}}\n"
    )
}

fn session(
    text: &str,
) -> (
    tempfile::TempDir,
    PathBuf,
    std::sync::MutexGuard<'static, ()>,
) {
    let serial = CACHE_TESTS.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    fs::write(&path, text).unwrap();
    (tmp, path, serial)
}

fn cached(path: &Path, run: &Run) -> Result<Option<Usage>> {
    transcript_cache::with_lines(path, |lines| {
        parse_session_entries(
            lines,
            run,
            false,
            &Config::default_for(Path::new("/fixture")),
        )
    })
}

fn append(path: &Path, text: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn appended_usage_is_decoded_once_and_matches_full_reparse() {
    // Given a cached transcript and one newly appended response.
    let first = format!("{HEADER}{}", response("a", 1, 2));
    let (_tmp, path, _serial) = session(&first);
    cached(&path, &run()).unwrap();
    let before = transcript_cache::counts(&path);
    let next = response("b", 3, 4);
    append(&path, &next);
    // When refreshing the transcript repeatedly.
    let usage = cached(&path, &run()).unwrap().unwrap();
    let again = cached(&path, &run()).unwrap().unwrap();
    // Then only the appended line is decoded, and each response contributes once.
    assert_eq!(
        usage,
        parse_session(Cursor::new(first + &next), &run(), false)
            .unwrap()
            .unwrap()
    );
    assert_eq!((usage.input, usage.output, usage.entries.len()), (4, 6, 2));
    assert_eq!(again, usage);
    let after = transcript_cache::counts(&path);
    assert_eq!((after.0 - before.0, after.1 - before.1), (1, 1));
}

#[test]
fn partial_trailing_usage_is_replaced_when_completed() {
    // Given a cached incomplete response after a completed header.
    let line = response("a", 5, 8);
    let split = line.len() / 2;
    let (_tmp, path, _serial) = session(&format!("{HEADER}{}", &line[..split]));
    assert!(cached(&path, &run()).is_err());
    assert_eq!(
        transcript_cache::counts(&path).2,
        u64::try_from(HEADER.len()).unwrap()
    );
    append(&path, &line[split..]);
    // When the trailing line becomes complete.
    let usage = cached(&path, &run()).unwrap().unwrap();
    // Then neither a partial checkpoint nor an extra response survives.
    assert_eq!((usage.input, usage.output, usage.entries.len()), (5, 8, 1));
    assert_eq!(
        usage,
        parse_session(Cursor::new(format!("{HEADER}{line}")), &run(), false)
            .unwrap()
            .unwrap()
    );
}

#[test]
fn valid_unterminated_usage_remains_provisional_until_newline() {
    // Given valid JSON at EOF, which full parsing already includes.
    let line = response("a", 2, 3);
    let (_tmp, path, _serial) = session(&format!("{HEADER}{}", line.trim_end()));
    let first = cached(&path, &run()).unwrap().unwrap();
    append(&path, "\n");
    // When its terminating newline arrives.
    let usage = cached(&path, &run()).unwrap().unwrap();
    // Then the provisional entry is replaced rather than duplicated.
    assert_eq!(usage, first);
    assert_eq!(usage.entries.len(), 1);
}

#[test]
fn truncated_usage_discards_previous_transcript_entries() {
    // Given a transcript replaced in place with shorter content.
    let (_tmp, path, _serial) = session(&format!(
        "{HEADER}{}{}",
        response("a", 1, 2),
        response("b", 3, 4)
    ));
    cached(&path, &run()).unwrap();
    let replacement = format!("{HEADER}{}", response("c", 7, 9));
    fs::write(&path, &replacement).unwrap();
    // When refreshing after truncation.
    let usage = cached(&path, &run()).unwrap().unwrap();
    // Then the result contains only the new transcript.
    assert_eq!(
        usage,
        parse_session(Cursor::new(replacement), &run(), false)
            .unwrap()
            .unwrap()
    );
}

#[test]
fn replaced_usage_resets_even_with_identical_length_and_anchor() {
    // Given a new inode at the same path, with an unchanged last response.
    let suffix = response("b", 3, 4);
    let (tmp, path, _serial) = session(&format!("{HEADER}{}{suffix}", response("a", 1, 2)));
    cached(&path, &run()).unwrap();
    let replacement = format!("{HEADER}{}{suffix}", response("a", 8, 9));
    let other = tmp.path().join("replacement");
    fs::write(&other, &replacement).unwrap();
    fs::remove_file(&path).unwrap();
    fs::rename(other, &path).unwrap();
    // When the same-length replacement is refreshed.
    let usage = cached(&path, &run()).unwrap().unwrap();
    // Then identity, not the unchanged anchor, invalidates old entries.
    assert_eq!(
        usage,
        parse_session(Cursor::new(replacement), &run(), false)
            .unwrap()
            .unwrap()
    );
}

#[test]
fn regrown_usage_resets_when_previous_anchor_changes() {
    // Given truncate-and-regrow on the same inode beyond its previous size.
    let (_tmp, path, _serial) = session(&format!("{HEADER}{}", response("a", 1, 2)));
    cached(&path, &run()).unwrap();
    let replacement = format!("{HEADER}{}{}", response("a", 8, 9), response("b", 3, 4));
    fs::write(&path, &replacement).unwrap();
    // When size alone would otherwise look like an append.
    let usage = cached(&path, &run()).unwrap().unwrap();
    // Then the anchor invalidates stale responses.
    assert_eq!(
        usage,
        parse_session(Cursor::new(replacement), &run(), false)
            .unwrap()
            .unwrap()
    );
}

#[test]
fn unchanged_usage_skips_payload_reads_and_decoding() {
    // Given an already parsed transcript.
    let (_tmp, path, _serial) = session(OMP);
    let before = cached(&path, &run()).unwrap();
    let counts = transcript_cache::counts(&path);
    // When refreshing an unchanged identity, length and anchor.
    let after = cached(&path, &run()).unwrap();
    // Then only the bounded anchor check runs, not a transcript payload read.
    assert_eq!(after, before);
    assert_eq!(transcript_cache::counts(&path), counts);
}

#[test]
fn active_usage_reads_events_once_for_multiple_tasks() {
    // Given multiple active tasks and a task outside the board refresh cohort.
    let _serial = CACHE_TESTS.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    for status in [
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
    ] {
        let mut task = store.create("Usage", None, status, "").unwrap();
        let mut snapshot = run();
        snapshot.agent = "unsupported".into();
        snapshot.ended = None;
        task.runs.push(snapshot);
        store.save(&task).unwrap();
    }
    let before = EVENT_READS.get();
    // When the board refreshes usage.
    refresh_active_usage(&store);
    // Then exactly one event snapshot serves every active task.
    assert_eq!(EVENT_READS.get() - before, 1);
}

#[test]
fn cached_usage_reapplies_windows_models_prices_and_claude_triggers() {
    // Given cached Claude fragments and a later run window with different pricing.
    let (_tmp, path, _serial) = session(CLAUDE);
    let mut snapshot = run();
    let config = Config::default_for(Path::new("/fixture"));
    transcript_cache::with_lines(&path, |lines| {
        parse_session_entries(lines, &snapshot, true, &config)
    })
    .unwrap();
    snapshot.started = "2026-10-14T03:20:49Z".parse().unwrap();
    snapshot.model = Some("review-model".into());
    let config: Config = serde_json::from_value(serde_json::json!({
        "pricing":{"review-model":{"input":2.0,"output":8.0,"cache_read":0.5,"cache_write":3.0}}
    }))
    .unwrap();
    // When aggregating the same decoded lines for the review's current context.
    let usage = transcript_cache::with_lines(&path, |lines| {
        parse_session_entries(lines, &snapshot, true, &config)
    })
    .unwrap();
    // Then filtering, response dedupe, timing and pricing equal a full reparse.
    assert_eq!(
        usage,
        parse_session_priced(Cursor::new(CLAUDE), &snapshot, true, &config).unwrap()
    );
    assert_eq!(usage.unwrap().entries[0].latency_ms, Some(2221));
}

#[test]
fn unrelated_cwd_still_short_circuits_later_cached_errors() {
    // Given a different cwd followed by malformed complete JSON.
    let (_tmp, path, _serial) = session("{\"type\":\"session\",\"cwd\":\"/other\"}\nnot json\n");
    // When the cached entries are aggregated.
    let usage = cached(&path, &run()).unwrap();
    // Then the transcript contributes nothing, just as the full parser would.
    assert_eq!(usage, None);
}

#[path = "usage_cache_eviction_tests.rs"]
mod eviction;

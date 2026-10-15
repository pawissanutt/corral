use super::*;
use std::io::Cursor;

const OMO: &str = include_str!("../tests/fixtures/usage/omo.jsonl");
const OMP: &str = include_str!("../tests/fixtures/usage/omp.jsonl");
const CLAUDE: &str = include_str!("../tests/fixtures/usage/claude.jsonl");

fn run() -> Run {
    serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"omo", "worktree":"/fixture/project",
        "branch":"test", "base":"main", "base_sha":"abc",
        "started":"2026-09-01T00:00:00Z", "ended":"2026-11-01T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn profile_names_resolve_to_their_harnesses() {
    // Given named presets for each supported transcript format.
    let mut config = Config::default_for(Path::new("/fixture"));
    for (name, harness) in [
        ("opus", "omo"),
        ("omp-opus", "omp"),
        ("claude-opus", "claude"),
    ] {
        config.profiles.insert(
            name.into(),
            corral_core::Profile {
                harness: harness.into(),
                model: None,
                effort: None,
                args: vec![],
            },
        );
        let mut run = run();
        run.agent = name.into();
        // When resolving a historical run without a recorded harness.
        let resolved = run_harness(&run, &config);
        // Then the preset's harness selects the transcript format.
        assert_eq!(resolved, harness);
    }
}

#[test]
fn recorded_harness_wins_over_current_profile() {
    // Given a profile whose current harness differs from the recorded run.
    let config = Config::default_for(Path::new("/fixture"));
    let mut run = run();
    run.harness = Some("omp".into());
    // When resolving the run.
    let resolved = run_harness(&run, &config);
    // Then the historical harness is authoritative.
    assert_eq!(resolved, "omp");
}

#[test]
fn legacy_harness_name_survives_without_a_profile() {
    // Given a legacy run with no profile entry.
    let mut config = Config::default_for(Path::new("/fixture"));
    config.profiles.clear();
    let run = run();
    // When resolving the run.
    let resolved = run_harness(&run, &config);
    // Then the agent field remains the harness.
    assert_eq!(resolved, "omo");
}

#[test]
fn unknown_profile_keeps_unsupported_harness_error() {
    // Given a run with neither a recorded harness nor a configured profile.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut run = run();
    run.agent = "unknown-profile".into();
    // When collecting usage.
    collect(&mut run, &store);
    // Then no usage is invented and the unsupported harness is identified.
    assert_eq!(run.usage, None);
    assert_eq!(
        run.usage_note.as_deref(),
        Some("unsupported harness unknown-profile")
    );
}

#[test]
fn captured_omo_usage_keeps_cache_and_reported_cost() {
    // Given captured OmO metadata.
    let run = run();
    // When parsed.
    let usage = parse_session(Cursor::new(OMO), &run, false)
        .unwrap()
        .unwrap();
    // Then all four counters and the reported dollar cost survive.
    assert_eq!(
        (
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write
        ),
        (28089, 181, 3968, 0)
    );
    assert!((usage.cost_usd.unwrap() - 0.1167696).abs() < 1e-10);
}

#[test]
fn captured_omp_usage_sums_assistant_entries_only() {
    // Given a session with user, assistant, and tool-result entries.
    let run = run();
    // When parsed.
    let usage = parse_session(Cursor::new(OMP), &run, false)
        .unwrap()
        .unwrap();
    // Then both assistant responses contribute exactly once.
    assert_eq!((usage.input, usage.output), (10955, 260));
    assert!((usage.cost_usd.unwrap() - 0.00177325).abs() < 1e-10);
}

#[test]
fn captured_claude_fragments_count_one_provider_response() {
    // Given three fragments with the same provider message id.
    let run = run();
    // When parsed.
    let usage = parse_session(Cursor::new(CLAUDE), &run, true)
        .unwrap()
        .unwrap();
    // Then usage is not tripled, and unreported cost stays unknown.
    assert_eq!(
        (
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write
        ),
        (2, 331, 9103, 1485)
    );
    assert_eq!(usage.cost_usd, None);
}

#[test]
fn resumed_session_only_counts_usage_inside_run_window() {
    // Given a resumed session with a header before the run and responses around its bounds.
    let mut run = run();
    run.started = "2026-09-28T04:51:51Z".parse().unwrap();
    run.ended = Some("2026-09-28T04:51:52Z".parse().unwrap());
    // When parsed.
    let usage = parse_session(Cursor::new(OMP), &run, false)
        .unwrap()
        .unwrap();
    // Then only the overlapping response is measured.
    assert_eq!((usage.input, usage.output), (6526, 121));
}

#[test]
fn another_worktree_is_not_attributed_to_this_run() {
    // Given a transcript from a different cwd.
    let mut run = run();
    run.worktree = "/another/project".into();
    // When parsed.
    let usage = parse_session(Cursor::new(OMP), &run, false).unwrap();
    // Then it contributes no usage.
    assert_eq!(usage, None);
}

#[test]
fn partly_reported_cost_is_not_presented_as_full_run_cost() {
    // Given two valid responses, only one with cost.
    let text = format!("{OMP}\n{}", OMO.replace(r#""cost":{"input":0.112356,"output":0.0036200000000000004,"cacheRead":0.0007936000000000001,"cacheWrite":0,"total":0.1167696}"#, r#""cost":null"#));
    // When parsed.
    let usage = parse_session(Cursor::new(text), &run(), false)
        .unwrap()
        .unwrap();
    // Then tokens are summed but the cost is unknown.
    assert_eq!(usage.input, 39044);
    assert_eq!(usage.cost_usd, None);
}

#[test]
fn malformed_usage_is_not_reported_as_partial_success() {
    // Given a valid response followed by an unknown usage format.
    let text = format!(
        "{OMP}\n{}",
        r#"{"type":"message","timestamp":"2026-10-01T00:00:00Z","message":{"role":"assistant","usage":{"tokens":42}}}"#
    );
    // When parsed.
    let result = parse_session(Cursor::new(text), &run(), false);
    // Then the incomplete measurement is rejected.
    assert!(result.is_err());
}

#[test]
fn absent_or_unknown_formats_leave_usage_empty_with_note() {
    // Given a configured OmO session root containing an unknown format.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
    let mut run = run();
    let dir = tmp
        .path()
        .join("omo/sessions")
        .join(crate::omo::encode_cwd(&run.worktree));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("unknown.jsonl"), "not json\n").unwrap();
    // When best-effort collection runs.
    collect(&mut run, &store);
    // Then the caller still proceeds, with no fabricated usage and a diagnostic.
    assert!(run.usage.is_none());
    assert!(run.usage_note.is_some());
}

#[test]
fn stats_backfills_usage_and_persists_it_without_losing_run_data() {
    // Given a legacy run and captured session under the configured OmO root.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
    let mut task = store
        .create("Measure", None, corral_core::Status::Done, "")
        .unwrap();
    task.runs.push(run());
    store.save(&task).unwrap();
    let dir = tmp
        .path()
        .join("omo/sessions")
        .join(crate::omo::encode_cwd(&task.runs[0].worktree));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.jsonl"), OMO).unwrap();
    // When stats lazily collects usage.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then the run snapshot is persisted and contributes to stats.
    let saved = store.get(&task.id).unwrap();
    assert_eq!(saved.runs[0].usage.as_ref().unwrap().input, 28089);
    assert_eq!(saved.runs[0].base_sha, "abc");
    assert_eq!(rows[0].total_tokens, Some(32238));
}

#[test]
fn stats_backfills_profile_runs_with_unsupported_harness_notes() {
    // Given a profile run previously rejected by usage collection.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
    store.config.profiles.insert(
        "opus".into(),
        corral_core::Profile {
            harness: "omo".into(),
            model: None,
            effort: None,
            args: vec![],
        },
    );
    let mut task = store
        .create("Measure profile", None, corral_core::Status::Done, "")
        .unwrap();
    let mut run = run();
    run.agent = "opus".into();
    run.usage_note = Some("unsupported harness opus".into());
    let mut stale = run.clone();
    stale.id = "R2".into();
    stale.usage = Some(Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cost_usd: None,
    });
    task.runs.push(run);
    task.runs.push(stale);
    store.save(&task).unwrap();
    let dir = tmp
        .path()
        .join("omo/sessions")
        .join(crate::omo::encode_cwd(&task.runs[0].worktree));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.jsonl"), OMO).unwrap();
    // When stats retries collection from the fixture transcript.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then the profile receives usage and its stale diagnostic is cleared.
    let saved = store.get(&task.id).unwrap();
    assert_eq!(saved.runs[0].usage.as_ref().unwrap().input, 28089);
    assert_eq!(saved.runs[0].usage_note, None);
    assert_eq!(saved.runs[1].usage.as_ref().unwrap().input, 28089);
    assert_eq!(saved.runs[1].usage_note, None);
    assert_eq!(rows[0].group, "opus");
    assert_eq!(rows[0].total_tokens, Some(64476));
}

#[test]
fn discovery_handles_omp_project_directory_and_ignores_artifacts() {
    // Given the legacy omp directory layout and a non-transcript artifacts directory.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("-fixture-project");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.jsonl"), OMP).unwrap();
    let artifacts = tmp.path().join("session-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join("journal.jsonl"), "not a transcript").unwrap();
    // When discovered by cwd and time.
    let usage = collect_from(tmp.path(), &run(), false).unwrap();
    // Then the transcript, not its artifacts, supplies the measurement.
    assert_eq!(usage.input, 10955);
}

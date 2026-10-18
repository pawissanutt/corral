use super::*;
use std::io::Cursor;

const OMO: &str = include_str!("../tests/fixtures/usage/omo.jsonl");
const OMP: &str = include_str!("../tests/fixtures/usage/omp.jsonl");
const CLAUDE: &str = include_str!("../tests/fixtures/usage/claude.jsonl");

fn parse_session(reader: impl BufRead, run: &Run, claude: bool) -> Result<Option<Usage>> {
    parse_session_priced(
        reader,
        run,
        claude,
        &Config::default_for(Path::new("/fixture")),
    )
}

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
                vendor: None,
                escalate_to: None,
                metadata: None,
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
fn captured_harness_responses_retain_timestamp_and_preceding_entry_latency() {
    // Given each harness's captured session, including Claude's repeated fragments.
    for (text, claude, expected) in [
        (OMO, false, vec![("2026-10-15T10:00:55.250Z", 11956)]),
        (
            OMP,
            false,
            vec![
                ("2026-09-28T04:51:50.982Z", 1068),
                ("2026-09-28T04:51:51.908Z", 912),
            ],
        ),
        (CLAUDE, true, vec![("2026-10-14T03:20:50.090Z", 757)]),
    ] {
        // When response evidence is parsed and fragments are deduplicated.
        let usage = parse_session(Cursor::new(text), &run(), claude)
            .unwrap()
            .unwrap();
        // Then the response's timestamp and immediate predecessor determine timing.
        let timing: Vec<_> = usage
            .entries
            .iter()
            .map(|entry| (entry.at, entry.latency_ms))
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(at, ms)| (Some(at.parse::<Timestamp>().unwrap()), Some(ms)))
            .collect();
        assert_eq!(timing, expected);
    }
}

#[test]
fn missing_or_backwards_predecessor_leaves_latency_unknown() {
    // Given responses with no predecessor, an untimed predecessor, or a clock regression.
    for predecessor in [
        "",
        "{\"type\":\"message\",\"message\":{\"role\":\"user\"}}\n",
        "{\"type\":\"message\",\"timestamp\":\"2026-10-01T00:00:02Z\",\"message\":{\"role\":\"user\"}}\n",
    ] {
        let text = format!(
            "{{\"type\":\"session\",\"cwd\":\"/fixture/project\"}}\n{predecessor}\
             {{\"type\":\"message\",\"timestamp\":\"2026-10-01T00:00:01Z\",\
             \"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":1,\"output\":2}}}}}}\n"
        );
        // When extracting usage.
        let usage = parse_session(Cursor::new(text), &run(), false)
            .unwrap()
            .unwrap();
        // Then the response time remains useful without fabricated elapsed time.
        assert_eq!(
            usage.entries[0].at,
            Some("2026-10-01T00:00:01Z".parse().unwrap())
        );
        assert_eq!(usage.entries[0].latency_ms, None);
    }
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
    assert_eq!(usage.entries[0].latency_ms, Some(912));
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
    assert_eq!(rows[0].no_timing_entries, Some(0));
}

#[test]
fn refresh_backfills_completed_response_and_reviewer_timing_without_changing_totals() {
    // Given completed runs with stored untimed entries and a retained transcript.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
    let mut task = store
        .create("Backfill timing", None, Status::Done, "")
        .unwrap();
    let mut parent = run();
    let mut old_usage = parse_session(Cursor::new(OMO), &parent, false)
        .unwrap()
        .unwrap();
    for entry in &mut old_usage.entries {
        entry.at = None;
        entry.latency_ms = None;
    }
    parent.usage = Some(old_usage.clone());
    let mut reviewer = parent.clone();
    reviewer.id = "R1-review1".into();
    parent.review_runs.push(reviewer);
    let dir = tmp
        .path()
        .join("omo/sessions")
        .join(crate::omo::encode_cwd(&parent.worktree));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.jsonl"), OMO).unwrap();
    task.runs.push(parent);
    store.save(&task).unwrap();
    // When refreshing saved evidence.
    refresh_usage(&store, &task.id).unwrap();
    // Then both run types gain timing while all token and dollar evidence is unchanged.
    let saved = store.get(&task.id).unwrap();
    for run in [&saved.runs[0], &saved.runs[0].review_runs[0]] {
        let mut usage = run.usage.clone().unwrap();
        assert_eq!(
            usage.entries[0].at,
            Some("2026-10-15T10:00:55.250Z".parse().unwrap())
        );
        assert_eq!(usage.entries[0].latency_ms, Some(11956));
        for entry in &mut usage.entries {
            entry.at = None;
            entry.latency_ms = None;
        }
        assert_eq!(usage, old_usage);
    }
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
            vendor: None,
            escalate_to: None,
            metadata: None,
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
        ..Usage::default()
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
    let usage = collect_from(tmp.path(), &run(), false, &Config::default_for(tmp.path())).unwrap();
    // Then the transcript, not its artifacts, supplies the measurement.
    assert_eq!(usage.input, 10955);
}

#[test]
fn subscription_fixture_estimates_without_reported_dollars() {
    // Given an omp/Codex subscription response without any cost field.
    let config: Config = serde_json::from_value(serde_json::json!({
        "pricing":{"gpt-subscription-fixture":{
            "input":2.0,"output":10.0,"cache_read":0.5,"cache_write":3.0
        }}
    }))
    .unwrap();
    // When its per-response counters are priced.
    let usage = parse_session_priced(
        Cursor::new(include_str!(
            "../tests/fixtures/usage/codex-subscription.jsonl"
        )),
        &run(),
        false,
        &config,
    )
    .unwrap()
    .unwrap();
    // Then the API-equivalent estimate exists independently of reported dollars.
    assert_eq!(usage.reported_usd, None);
    assert!((usage.estimated_usd.unwrap() - 0.00455).abs() < 1e-10);
    assert_eq!(usage.cost_basis, Some(corral_core::CostBasis::Estimated));
}

#[test]
fn claude_costusd_is_preserved_next_to_estimate() {
    // Given Claude fragments with a reported cost and a recorded run model.
    let text = CLAUDE.replace(
        r#""type":"assistant","#,
        r#""type":"assistant","costUSD":0.25,"#,
    );
    let mut run = run();
    run.model = Some("claude-fixture".into());
    let config: Config = serde_json::from_value(serde_json::json!({
        "pricing":{"claude-fixture":{
            "input":4.0,"output":20.0,"cache_read":0.2,"cache_write":5.0
        }}
    }))
    .unwrap();
    // When repeated provider snapshots are deduplicated.
    let usage = parse_session_priced(Cursor::new(text), &run, true, &config)
        .unwrap()
        .unwrap();
    // Then both reported and estimated totals survive exactly once.
    assert!((usage.reported_usd.unwrap() - 0.25).abs() < 1e-10);
    assert!((usage.estimated_usd.unwrap() - 0.0158736).abs() < 1e-10);
    assert_eq!(usage.entries.len(), 1);
    assert_eq!(usage.cost_basis, Some(corral_core::CostBasis::Reported));
}

#[test]
fn omp_home_relative_bucket_is_discovered_using_header_cwd() {
    // Given omp 18's real home-relative layout and a transcript with absolute cwd.
    let tmp = tempfile::tempdir().unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let mut run = run();
    run.worktree = home.join("miti-workspace/corral/.worktrees/T-030");
    let dir = tmp.path().join("-miti-workspace-corral-.worktrees-T-030");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("session.jsonl"),
        OMP.replace("/fixture/project", &run.worktree.to_string_lossy()),
    )
    .unwrap();
    // When the collector applies directory discovery and authoritative cwd/time checks.
    let usage = collect_from(tmp.path(), &run, false, &Config::default_for(tmp.path())).unwrap();
    // Then both assistant responses, but no unrelated cwd, are attributed.
    assert_eq!(usage.total_tokens(), 11215);
}

#[test]
fn stored_model_entries_reprice_without_transcript_access() {
    // Given a completed run with model-specific breakdowns but no transcript on disk.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("missing-transcripts"));
    store.config.pricing.insert(
        "known".into(),
        corral_core::Price {
            input: 3.0,
            output: 9.0,
            cache_read: None,
            cache_write: None,
            aliases: vec![],
        },
    );
    let mut task = store
        .create("Reprice", None, corral_core::Status::Done, "")
        .unwrap();
    let mut run = run();
    run.usage = Some(Usage::from_entries(vec![UsageEntry {
        model: Some("known".into()),
        input: 1_000_000,
        ..UsageEntry::default()
    }]));
    task.runs.push(run);
    store.save(&task).unwrap();
    // When stats refreshes stored estimates.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then the current table, not a stale computed value, determines the estimate.
    let task = store.get(&task.id).unwrap();
    assert!((task.runs[0].usage.as_ref().unwrap().estimated_usd.unwrap() - 3.0).abs() < 1e-10);
    assert_eq!(task.runs[0].usage_note, None);
    let usage = task.runs[0].usage.as_ref().unwrap();
    assert_eq!(
        (usage.entries[0].at, usage.entries[0].latency_ms),
        (None, None)
    );
    assert_eq!(rows[0].no_timing_entries, Some(1));
    assert_eq!(rows[0].total_tokens, Some(1_000_000));
}

#[test]
fn legacy_aggregate_without_transcript_counts_as_no_timing() {
    // Given legacy aggregate usage whose transcript has been removed.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("missing-transcripts"));
    let mut task = store
        .create("Legacy timing", None, Status::Done, "")
        .unwrap();
    let mut run = run();
    run.usage = Some(Usage {
        input: 42,
        cost_usd: Some(0.25),
        ..Usage::default()
    });
    task.runs.push(run);
    store.save(&task).unwrap();
    // When stats refreshes and combines historical usage.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then tokens and reported cost survive, with unknown timing counted separately.
    let saved = store.get(&task.id).unwrap();
    let combined = saved.total_usage().unwrap();
    assert_eq!(
        (combined.entries[0].at, combined.entries[0].latency_ms),
        (None, None)
    );
    assert_eq!(rows[0].no_timing_entries, Some(1));
    assert_eq!(rows[0].total_tokens, Some(42));
    assert!((rows[0].total_cost_usd.unwrap() - 0.25).abs() < 1e-10);
}

#[test]
fn model_change_prices_each_response_with_its_own_model() {
    // Given a session switching between a priced model and an unpriced one.
    let text = concat!(
        "{\"type\":\"session\",\"cwd\":\"/fixture/project\"}\n",
        "{\"type\":\"model_change\",\"modelId\":\"known\"}\n",
        "{\"type\":\"message\",\"id\":\"a\",\"timestamp\":\"2026-10-01T00:00:01Z\",",
        "\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":1000000,\"output\":0}}}\n",
        "{\"type\":\"model_change\",\"modelId\":\"unknown\"}\n",
        "{\"type\":\"message\",\"id\":\"b\",\"timestamp\":\"2026-10-01T00:00:02Z\",",
        "\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":99,\"output\":1}}}\n"
    );
    let config: Config = serde_json::from_value(serde_json::json!({
        "pricing":{"known":{"input":2.0,"output":10.0}}
    }))
    .unwrap();
    // When parsing model changes before their respective responses.
    let usage = parse_session_priced(Cursor::new(text), &run(), false, &config)
        .unwrap()
        .unwrap();
    // Then only the first response is priced; no run-wide model guesses overwrite the switch.
    assert!((usage.estimated_usd.unwrap() - 2.0).abs() < 1e-10);
    assert_eq!(usage.unpriced_tokens, 100);
    assert_eq!(usage.entries[1].model.as_deref(), Some("unknown"));
}

#[test]
fn legacy_review_events_backfill_every_reviewer_window_once() {
    // Given two old reviewer launches whose snapshots predate reviewer accounting.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Reviews", None, corral_core::Status::Done, "")
        .unwrap();
    let mut parent = run();
    let events: Vec<Event> = serde_json::from_value(serde_json::json!([
        {"task":task.id,"kind":"review_started","ts":"2026-10-01T00:00:00Z",
         "detail":{"run":"R1","reviewer":"omo"}},
        {"task":task.id,"kind":"verdict","ts":"2026-10-01T00:01:00Z",
         "detail":{"run":"R1","verdict":"changes"}},
        {"task":task.id,"kind":"review_started","ts":"2026-10-01T01:00:00Z",
         "detail":{"run":"R1","reviewer":"pm"}},
        {"task":task.id,"kind":"verdict","ts":"2026-10-01T01:01:00Z",
         "detail":{"run":"R1","verdict":"approve"}}
    ]))
    .unwrap();
    // When reconstructing historical sessions, including an idempotent second refresh.
    backfill_reviews(&mut parent, &store, &task.id, &events);
    backfill_reviews(&mut parent, &store, &task.id, &events);
    // Then both bounded sessions survive, with PM's actual workspace cwd.
    assert_eq!(parent.review_runs.len(), 2);
    assert_eq!(parent.review_runs[0].started, events[0].ts);
    assert_eq!(parent.review_runs[0].ended, Some(events[1].ts));
    assert_eq!(parent.review_runs[1].worktree, store.root());
    assert_eq!(parent.review_runs[1].ended, Some(events[3].ts));
}

#[test]
fn owner_approval_closes_an_existing_reviewer_without_verdict() {
    // Given a saved active reviewer and an owner approval at a fixed timestamp.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Owner approval", None, corral_core::Status::Done, "")
        .unwrap();
    let mut parent = run();
    let mut reviewer = run();
    reviewer.id = "R1-review1".into();
    reviewer.agent = "unsupported".into();
    reviewer.started = "2026-10-01T00:00:00Z".parse().unwrap();
    reviewer.ended = None;
    reviewer.usage = Some(Usage {
        input: 42,
        ..Usage::default()
    });
    parent.review_runs.push(reviewer);
    task.runs.push(parent);
    store.save(&task).unwrap();
    let event = serde_json::json!({
        "task":task.id,"kind":"approved","ts":"2026-10-01T00:01:00Z","detail":{"run":"R1"}
    });
    fs::write(store.dir().join("events.jsonl"), format!("{event}\n")).unwrap();
    // When stats refreshes the accounting children.
    refresh_usage(&store, &task.id).unwrap();
    // Then the reviewer cannot accumulate unrelated post-approval usage.
    let saved = store.get(&task.id).unwrap();
    let reviewer = &saved.runs[0].review_runs[0];
    assert_eq!(
        reviewer.ended,
        Some("2026-10-01T00:01:00Z".parse().unwrap())
    );
    assert_eq!(reviewer.usage.as_ref().unwrap().total_tokens(), 42);
}

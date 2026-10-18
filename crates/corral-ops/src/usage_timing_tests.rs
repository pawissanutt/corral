use super::*;

fn untimed_sessions(
    transcripts: &[&str],
) -> (tempfile::TempDir, Store, corral_core::Task, PathBuf, Usage) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
    let mut task = store
        .create("Timing refresh", None, Status::Done, "")
        .unwrap();
    let mut run = run();
    let dir = tmp
        .path()
        .join("omo/sessions")
        .join(crate::omo::encode_cwd(&run.worktree));
    fs::create_dir_all(&dir).unwrap();
    for (index, text) in transcripts.iter().enumerate() {
        fs::write(dir.join(format!("session-{index}.jsonl")), text).unwrap();
    }
    collect(&mut run, &store);
    let usage = run.usage.as_mut().unwrap();
    for entry in &mut usage.entries {
        entry.at = None;
        entry.latency_ms = None;
    }
    let original = usage.clone();
    task.runs.push(run);
    store.save(&task).unwrap();
    (tmp, store, task, dir, original)
}

#[test]
fn partial_transcript_backfill_preserves_missing_sessions_and_totals() {
    // Given untimed evidence from two sessions, with only the OmO transcript retained.
    let (_tmp, store, task, dir, original) = untimed_sessions(&[OMO, OMP]);
    fs::remove_file(dir.join("session-1.jsonl")).unwrap();
    // When stats requests a timing refresh.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then the retained response gains timing without discarding either missing response.
    let saved = store.get(&task.id).unwrap();
    let mut usage = saved.runs[0].usage.clone().unwrap();
    assert_eq!(usage.entries.len(), 3);
    assert_eq!(
        usage.entries[0].at,
        Some("2026-10-15T10:00:55.250Z".parse().unwrap())
    );
    assert_eq!(usage.entries[0].latency_ms, Some(11956));
    assert!(usage.entries[1..].iter().all(|entry| entry.at.is_none()));
    assert_eq!(rows[0].no_timing_entries, Some(2));
    assert_eq!(rows[0].total_tokens, Some(43453));
    assert!((rows[0].total_cost_usd.unwrap() - 0.11854285).abs() < 1e-10);
    for entry in &mut usage.entries {
        entry.at = None;
        entry.latency_ms = None;
    }
    assert_eq!(usage, original);
}

#[test]
fn duplicate_saved_responses_remain_untimed_when_matching_is_ambiguous() {
    // Given the reviewer's two identical OmO sessions, one of which is now gone.
    let later = OMO.replace("2026-10-15", "2026-10-16");
    let (_tmp, store, task, dir, original) = untimed_sessions(&[OMO, &later]);
    fs::remove_file(dir.join("session-1.jsonl")).unwrap();
    // When stats refreshes usage.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then no arbitrary timestamp is assigned to indistinguishable stored responses.
    let saved = store.get(&task.id).unwrap();
    assert_eq!(saved.runs[0].usage.as_ref(), Some(&original));
    assert_eq!(rows[0].no_timing_entries, Some(2));
    assert_eq!(rows[0].total_tokens, Some(64476));
    assert!((rows[0].total_cost_usd.unwrap() - 0.2335392).abs() < 1e-10);
}

#[test]
fn duplicate_transcript_responses_do_not_guess_a_saved_timestamp() {
    // Given one saved response and an additional indistinguishable transcript response.
    let (_tmp, store, task, dir, original) = untimed_sessions(&[OMO]);
    fs::write(
        dir.join("extra.jsonl"),
        OMO.replace("2026-10-15", "2026-10-16"),
    )
    .unwrap();
    // When stats refreshes timing.
    let rows = crate::stats(&store, crate::StatsBy::Profile, None).unwrap();
    // Then ambiguous transcript evidence neither fabricates timing nor changes totals.
    let saved = store.get(&task.id).unwrap();
    assert_eq!(saved.runs[0].usage.as_ref(), Some(&original));
    assert_eq!(rows[0].no_timing_entries, Some(1));
    assert_eq!(rows[0].total_tokens, Some(32238));
}

#[test]
fn claude_tool_trigger_is_retained_across_fragments_and_run_window() {
    // Given a tool result followed by fragments straddling the run's start.
    let text = concat!(
        "{\"type\":\"user\",\"cwd\":\"/fixture/project\",\"timestamp\":\"2026-10-01T00:00:01Z\",",
        "\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"content\":\"ok\"}]}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-10-01T00:00:02Z\",",
        "\"message\":{\"id\":\"response\",\"role\":\"assistant\",\"usage\":{\"input\":10,\"output\":1}}}\n",
        "{\"type\":\"user\",\"timestamp\":\"2026-10-01T00:00:03Z\",\"message\":{\"role\":\"user\"}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-10-01T00:00:04.250Z\",",
        "\"message\":{\"id\":\"response\",\"role\":\"assistant\",\"usage\":{\"input\":10,\"output\":5}}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-10-01T00:00:05Z\",",
        "\"message\":{\"id\":\"next-response\",\"role\":\"assistant\",\"usage\":{\"input\":20,\"output\":2}}}\n"
    );
    let mut run = run();
    run.started = "2026-10-01T00:00:03Z".parse().unwrap();
    // When reading the last fragment in the run's window.
    let usage = parse_session(Cursor::new(text), &run, true)
        .unwrap()
        .unwrap();
    // Then that message keeps its original tool trigger; a new message uses the new input.
    let response = usage
        .entries
        .iter()
        .find(|entry| entry.input == 10)
        .unwrap();
    assert_eq!(
        response.at,
        Some("2026-10-01T00:00:04.250Z".parse().unwrap())
    );
    assert_eq!(response.latency_ms, Some(3250));
    let next = usage
        .entries
        .iter()
        .find(|entry| entry.input == 20)
        .unwrap();
    assert_eq!(next.latency_ms, Some(2000));
    assert_eq!(usage.output, 7);
}

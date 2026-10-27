use super::*;

const OVERLOADED: &str = include_str!("../tests/fixtures/provider-stall/overloaded.jsonl");
const RECOVERED: &str = include_str!("../tests/fixtures/provider-stall/recovered.jsonl");
const CREDENTIALS: &str = include_str!("../tests/fixtures/provider-stall/credentials.jsonl");

fn ts(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn stall(at: &str) -> Stall {
    Stall {
        at: ts(at),
        message: "Codex error: Our servers are currently overloaded.".into(),
        class: AvailabilityClass::Transient {
            retry_after_secs: None,
        },
    }
}

#[test]
fn transient_errors_are_told_apart_from_permanent_ones() {
    for message in [
        "Codex error: Our servers are currently overloaded. Please try again later.",
        "429 Too Many Requests",
        "rate limit exceeded",
        "upstream returned 503",
        "Bad Gateway",
    ] {
        assert!(is_transient(message), "{message}");
    }
    for message in [
        "No credential slots available",
        "invalid api key",
        "context length exceeded (15000 tokens)",
    ] {
        assert!(!is_transient(message), "{message}");
    }
}

#[test]
fn recorded_logs_report_a_stall_only_while_the_error_is_the_last_turn() {
    let open = scan(OVERLOADED).unwrap();
    assert_eq!(open.at, ts("2026-10-15T09:10:00Z"));
    assert!(open.message.contains("currently overloaded"));
    assert_eq!(scan(RECOVERED), None);
    // Credentials are an availability class; only governed profiles act on it.
    assert_eq!(
        scan(CREDENTIALS).map(|stall| stall.class),
        Some(AvailabilityClass::CredentialUnavailable)
    );
    let retry_prompt_only = format!(
        "{OVERLOADED}{{\"type\":\"message\",\"timestamp\":\"2026-10-15T09:11:00.000Z\",\"message\":{{\"role\":\"user\",\"content\":[]}}}}\n{{\"type\":\"mess"
    );
    assert_eq!(scan(&retry_prompt_only), Some(open));
}

#[test]
fn backoff_retries_three_times_then_escalates_once() {
    let first = stall("2026-10-15T09:10:00Z");
    let mut state = None;
    let at = |secs: i64| first.at + SignedDuration::from_secs(secs);
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(59)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(60)),
        Step::Retry(1)
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(179)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(180)),
        Step::Retry(2)
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(479)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(480)),
        Step::Retry(3)
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(779)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(780)),
        Step::Escalate
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(900)),
        Step::Idle
    );
    let reason = state.as_ref().unwrap().reason();
    assert!(reason.starts_with("provider overloaded since "), "{reason}");
}

#[test]
fn a_busy_harness_defers_the_retry_and_recovery_clears_the_state() {
    let first = stall("2026-10-15T09:10:00Z");
    let at = |secs: i64| first.at + SignedDuration::from_secs(secs);
    let mut state = None;
    assert_eq!(
        advance_state(&mut state, Some(&first), false, at(600)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(601)),
        Step::Retry(1)
    );
    assert_eq!(
        advance_state(&mut state, None, true, at(602)),
        Step::Recovered
    );
    assert_eq!(state, None);
    assert_eq!(advance_state(&mut state, None, true, at(603)), Step::Idle);
}

#[test]
fn a_failed_retry_keeps_the_episode_but_a_failure_after_escalation_starts_a_new_one() {
    let first = stall("2026-10-15T09:10:00Z");
    let at = |secs: i64| first.at + SignedDuration::from_secs(secs);
    let mut state = None;
    assert_eq!(
        advance_state(&mut state, Some(&first), true, at(60)),
        Step::Retry(1)
    );
    let again = stall("2026-10-15T09:11:05Z");
    assert_eq!(
        advance_state(&mut state, Some(&again), true, at(70)),
        Step::Wait
    );
    let kept = state.clone().unwrap();
    assert_eq!(
        (kept.since, kept.error_at, kept.attempts),
        (first.at, again.at, 1)
    );
    state.as_mut().unwrap().escalated = true;
    let nudged = stall("2026-10-15T10:00:00Z");
    assert_eq!(
        advance_state(&mut state, Some(&nudged), true, nudged.at),
        Step::Wait
    );
    let fresh = state.unwrap();
    assert_eq!(
        (fresh.since, fresh.attempts, fresh.escalated),
        (nudged.at, 0, false)
    );
}

#[test]
fn a_working_final_retry_defers_escalation_unless_it_already_failed() {
    let first = stall("2026-10-15T09:10:00Z");
    let at = |secs: i64| first.at + SignedDuration::from_secs(secs);
    let mut state = None;
    for (secs, attempt) in [(60, 1), (180, 2), (480, 3)] {
        assert_eq!(
            advance_state(&mut state, Some(&first), true, at(secs)),
            Step::Retry(attempt)
        );
    }
    let mut still_working = state.clone();
    assert_eq!(
        advance_state(&mut still_working, Some(&first), false, at(2000)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut still_working, Some(&first), true, at(2001)),
        Step::Escalate
    );

    let failed_again = stall("2026-10-15T09:18:20Z");
    assert_eq!(
        advance_state(&mut state, Some(&failed_again), false, at(510)),
        Step::Wait
    );
    assert_eq!(
        advance_state(&mut state, Some(&failed_again), true, at(520)),
        Step::Escalate
    );
}

#[test]
fn launch_boundary_and_retry_budget_survive_restart_without_file_authority() {
    // Given an earlier harness launch and one already-consumed retry.
    let tmp = tempfile::tempdir().unwrap();
    let ctx =
        crate::resources::FixedResourceProbe::ctx(corral_core::Store::init(tmp.path()).unwrap());
    let launch = ts("2026-10-15T09:09:00Z");
    let first = stall("2026-10-15T09:10:00Z");
    let mut state = None;
    advance_state(&mut state, Some(&first), true, ts("2026-10-15T09:11:00Z"));
    save(&ctx, "T-001-R1", state.as_ref().unwrap()).unwrap();
    provider_session::record_launch(&ctx, "T-001", "R1", launch);
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "omo", "worktree": tmp.path(),
        "branch": "main", "base": "main", "base_sha": "fixture",
        "started": "2026-10-15T09:09:30Z"
    }))
    .unwrap();

    // When a new handle resumes the same run, then its boundary and remaining budget survive.
    let restarted =
        crate::resources::FixedResourceProbe::ctx(corral_core::Store::open(tmp.path()).unwrap());
    assert_eq!(
        provider_session::boundary(&restarted, "T-001", &run),
        launch
    );
    let mut resumed = load(&restarted, "T-001-R1");
    assert_eq!(resumed, state);
    assert_eq!(
        advance_state(&mut resumed, Some(&first), true, ts("2026-10-15T09:13:00Z")),
        Step::Retry(2)
    );
    for area in ["provider-session", "provider-stall"] {
        assert_eq!(
            restarted
                .store
                .record_row(area, "T-001-R1")
                .unwrap()
                .unwrap()
                .doc["doc_version"],
            1
        );
    }
    assert!(!restarted.store.state_dir().join("provider-stall").exists());
}

#[test]
fn malformed_canonical_provider_records_keep_the_previous_fail_soft_behavior() {
    // Given malformed records in both provider areas.
    let tmp = tempfile::tempdir().unwrap();
    let ctx =
        crate::resources::FixedResourceProbe::ctx(corral_core::Store::init(tmp.path()).unwrap());
    for area in ["provider-session", "provider-stall"] {
        ctx.store
            .save_record(
                area,
                "T-001-R1",
                &json!({"doc_version": 1, "value": "invalid"}),
            )
            .unwrap();
    }
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "omo", "worktree": tmp.path(),
        "branch": "main", "base": "main", "base_sha": "fixture",
        "started": "2026-10-15T09:09:30Z"
    }))
    .unwrap();
    // When reading, then launch defaults to run start and malformed stall state is absent.
    assert_eq!(provider_session::boundary(&ctx, "T-001", &run), run.started);
    assert!(load(&ctx, "T-001-R1").is_none());
}

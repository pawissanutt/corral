use super::*;

fn config() -> PmGuardCfg {
    PmGuardCfg {
        large_context_tokens: 100,
        min_cache_read_ratio: 0.5,
        consecutive_misses: 2,
        price_tier_input_tokens: 200,
        pause: true,
    }
}

fn observation(generation: u32, request: u32, total: u64, read: u64) -> Observation {
    Observation {
        generation,
        purpose: RequestPurpose::Conversation,
        cache_lineage: Some(format!("session-{generation}")),
        request: request.to_string(),
        response: format!("response-{request}"),
        model: Some("fixture".into()),
        provider: Some("fixture".into()),
        at: Timestamp::from_second(i64::from(request)).unwrap(),
        usage: RequestUsage {
            input: total - read,
            cache_read: read,
            cache_write: 0,
            output: 1,
        },
    }
}

#[test]
fn consecutive_large_misses_pause_and_report_once() {
    let mut guard = Guard::default();
    let mut reports = Vec::new();
    for request in 1..=12 {
        reports.extend(guard.observe(&config(), observation(1, request, 100, 0)));
        assert_eq!(guard.paused(), request >= 3);
    }
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, "pm_cache_miss_guard");
    assert_eq!(reports[0].1["total_input_tokens"], 100);
    assert_eq!(reports[0].1["cache_read_ratio"], 0.0);
    assert_eq!(guard.observations.len(), HISTORY);
    assert_eq!(guard.observations[0].request, "5");
    match guard.state {
        State::Paused { since, .. } => assert_eq!(since.as_second(), 3),
        State::Ok => panic!("guard must pause"),
    }
}

#[test]
fn intervening_hit_and_small_context_reset_miss_streaks() {
    for (total, read) in [(100, 50), (99, 0), (0, 0)] {
        let mut guard = Guard::default();
        guard.observe(&config(), observation(1, 0, 100, 100));
        assert!(
            guard
                .observe(&config(), observation(1, 1, 100, 0))
                .is_empty()
        );
        assert!(
            guard
                .observe(&config(), observation(1, 2, total, read))
                .is_empty()
        );
        assert_eq!(guard.consecutive_misses, 0);
        assert!(
            guard
                .observe(&config(), observation(1, 3, 100, 0))
                .is_empty()
        );
        assert!(!guard.paused());
        assert_eq!(guard.observe(&config(), observation(1, 4, 100, 0)).len(), 1);
        assert!(guard.paused());
    }
}

#[test]
fn small_contexts_never_count_as_misses() {
    let mut guard = Guard::default();
    for request in 1..=4 {
        assert!(
            guard
                .observe(&config(), observation(1, request, 99, 0))
                .is_empty()
        );
    }
    assert!(!guard.paused());
    assert_eq!(guard.consecutive_misses, 0);
}

#[test]
fn price_tier_reports_once_per_upward_crossing_without_pausing() {
    let mut guard = Guard::default();
    let mut reports = Vec::new();
    for (request, total) in [199, 200, 201, 199, 200].into_iter().enumerate() {
        reports.extend(guard.observe(&config(), observation(1, request as u32, total, total)));
    }
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .all(|(kind, _)| *kind == "pm_price_tier_crossed")
    );
    assert_eq!(reports[0].1["previous_input_tokens"], 199);
    assert_eq!(reports[0].1["price_tier_input_tokens"], 200);
    assert!(!guard.paused());
}

#[test]
fn generation_boundary_clears_pause_history_and_streak() {
    let mut guard = Guard::default();
    guard.observe(&config(), observation(1, 1, 100, 0));
    guard.observe(&config(), observation(1, 2, 100, 0));
    guard.observe(&config(), observation(1, 3, 100, 0));
    assert!(guard.paused());
    assert!(
        guard
            .observe(&config(), observation(2, 4, 200, 0))
            .is_empty()
    );
    assert!(!guard.paused());
    assert_eq!(guard.consecutive_misses, 0);
    assert_eq!(guard.observations.len(), 1);
    assert_eq!(guard.generation, 2);
}

#[test]
fn report_only_guard_reports_once_per_episode_and_never_pauses() {
    let mut cfg = config();
    cfg.pause = false;
    let mut guard = Guard::default();
    let mut reports = Vec::new();
    for (request, read) in [0, 0, 0, 100, 0, 0].into_iter().enumerate() {
        reports.extend(guard.observe(&cfg, observation(1, request as u32, 100, read)));
    }
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .all(|(kind, detail)| *kind == "pm_cache_miss_guard" && detail["paused"] == false)
    );
    assert!(!guard.paused());
}

#[test]
fn normalized_input_buckets_exclude_output_from_ratio() {
    let usage = RequestUsage {
        input: 30,
        cache_read: 50,
        cache_write: 20,
        output: 400,
    };
    assert_eq!(usage.total_input(), 100);
    assert_eq!(usage.ratio(), 0.5);
    assert!(usage.valid(500));
    assert!(!usage.valid(480));
}

#[test]
fn cold_conversation_summary_and_post_compaction_read_do_not_pause() {
    for post_compaction_read in [0, 100] {
        let mut guard = Guard::default();
        assert!(
            guard
                .observe(&config(), observation(1, 1, 100, 0))
                .is_empty()
        );
        let mut summary = observation(1, 2, 300, 0);
        summary.purpose = RequestPurpose::Summary;
        assert!(guard.observe(&config(), summary).is_empty());
        assert_eq!(guard.consecutive_misses, 0);
        assert_eq!(
            guard.last_conversation.as_ref().unwrap().total_input_tokens,
            100
        );
        assert!(
            guard
                .observe(&config(), observation(1, 3, 100, post_compaction_read))
                .is_empty()
        );
        assert!(!guard.paused());
        assert_eq!(guard.consecutive_misses, 0);
        assert_eq!(guard.observations[1].purpose, RequestPurpose::Summary);
        assert!(
            guard
                .observe(&config(), observation(1, 4, 100, 0))
                .is_empty()
        );
        assert_eq!(guard.observe(&config(), observation(1, 5, 100, 0)).len(), 1);
        assert!(guard.paused());
    }
}

#[test]
fn structural_requests_preserve_streak_and_conversation_ratio_history() {
    let mut guard = Guard::default();
    guard.observe(&config(), observation(1, 1, 100, 100));
    guard.observe(&config(), observation(1, 2, 100, 25));
    for (index, purpose) in [RequestPurpose::Other, RequestPurpose::Summary]
        .into_iter()
        .enumerate()
    {
        let mut structural = observation(1, index as u32 + 3, 300, 0);
        structural.purpose = purpose;
        assert!(guard.observe(&config(), structural).is_empty());
        assert_eq!(guard.consecutive_misses, 1);
        assert_eq!(
            guard.last_conversation.as_ref().unwrap().total_input_tokens,
            100
        );
        assert_eq!(
            guard.last_conversation.as_ref().unwrap().cache_read_ratio,
            0.25
        );
    }
    assert!(!guard.paused());
    // Only the next conversation read starts the new summary lineage.
    assert!(
        guard
            .observe(&config(), observation(1, 5, 100, 0))
            .is_empty()
    );
    assert_eq!(guard.consecutive_misses, 0);
}

#[test]
fn auxiliary_summary_does_not_restart_the_main_conversation_streak() {
    let mut guard = Guard::default();
    guard.observe(&config(), observation(1, 1, 100, 100));
    guard.observe(&config(), observation(1, 2, 100, 0));
    let mut auxiliary_summary = observation(1, 3, 300, 0);
    auxiliary_summary.purpose = RequestPurpose::Summary;
    auxiliary_summary.cache_lineage = Some("auxiliary-session".into());
    assert!(guard.observe(&config(), auxiliary_summary).is_empty());
    assert_eq!(guard.consecutive_misses, 1);
    assert_eq!(guard.observe(&config(), observation(1, 4, 100, 0)).len(), 1);
    assert!(guard.paused());
}

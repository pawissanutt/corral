use super::*;

#[test]
fn omitted_guard_settings_keep_safe_defaults() {
    let guard: PmGuardCfg = serde_json::from_str("{}").unwrap();
    assert_eq!(guard, PmGuardCfg::default());
    assert!(guard.problems().is_empty());
    assert_eq!(guard.large_context_tokens, 260_000);
    assert_eq!(guard.price_tier_input_tokens, 272_000);
    assert_eq!(guard.consecutive_misses, 2);
    assert_eq!(guard.min_cache_read_ratio, 0.5);
    assert!(guard.pause);
}

#[test]
fn guard_ranges_reject_zero_thresholds_and_invalid_ratios() {
    let invalid: &[fn(&mut PmGuardCfg)] = &[
        |g| g.large_context_tokens = 0,
        |g| g.large_context_tokens = 9_007_199_254_740_992,
        |g| g.price_tier_input_tokens = 0,
        |g| g.price_tier_input_tokens = 9_007_199_254_740_992,
        |g| g.consecutive_misses = 0,
        |g| g.min_cache_read_ratio = -0.1,
        |g| g.min_cache_read_ratio = 1.1,
        |g| g.min_cache_read_ratio = f64::NAN,
        |g| g.min_cache_read_ratio = f64::INFINITY,
    ];
    for update in invalid {
        let mut guard = PmGuardCfg::default();
        update(&mut guard);
        assert_eq!(guard.problems().len(), 1);
    }
    for ratio in [0.0, 1.0] {
        let guard = PmGuardCfg {
            min_cache_read_ratio: ratio,
            ..PmGuardCfg::default()
        };
        assert!(guard.problems().is_empty());
    }
}

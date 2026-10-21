use super::*;

fn close(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("value present");
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

fn entry(output: u64, latency_ms: Option<u64>, estimated_usd: Option<f64>) -> UsageEntry {
    UsageEntry {
        output,
        latency_ms,
        estimated_usd,
        ..Default::default()
    }
}

fn finish(entries: &[UsageEntry]) -> Throughput {
    let mut totals = ThroughputTotals::default();
    for entry in entries {
        totals.add(entry);
    }
    totals.finish()
}

#[test]
fn percentile_interpolates_between_ranks() {
    let ten: Vec<f64> = (1..=10).map(f64::from).collect();
    close(percentile(&ten, 0), 1.0);
    close(percentile(&ten, 50), 5.5);
    close(percentile(&ten, 90), 9.1);
    close(percentile(&ten, 100), 10.0);
    close(percentile(&[1.0, 2.0, 3.0, 4.0], 50), 2.5);
}

#[test]
fn percentile_handles_one_sample_and_none() {
    close(percentile(&[4.0], 50), 4.0);
    close(percentile(&[4.0], 90), 4.0);
    assert_eq!(percentile(&[], 50), None);
}

#[test]
fn rates_use_timed_entries_and_count_every_exclusion() {
    // Given: 50, 150 and 50 tok/s responses plus missing and zero latency.
    let t = finish(&[
        entry(100, Some(2000), None),
        entry(300, Some(2000), None),
        entry(200, Some(4000), None),
        entry(500, None, None),
        entry(500, Some(0), None),
    ]);
    // Then: only timed entries are samples and each exclusion is counted once.
    assert_eq!((t.samples, t.excluded), (3, 2));
    close(t.p50_tokens_per_second, 50.0);
    close(t.p90_tokens_per_second, 130.0);
    close(Some(t.active_seconds), 8.0);
}

#[test]
fn only_untimed_entries_leave_rates_unknown() {
    let t = finish(&[entry(10, None, Some(1.0))]);
    assert_eq!((t.samples, t.excluded), (0, 1));
    assert_eq!(t.p50_tokens_per_second, None);
    assert_eq!(t.estimated_usd_per_active_hour, None);
}

#[test]
fn dollars_per_active_hour_use_priced_samples_only() {
    // Given: two priced half-hour responses and one unpriced hour-long response.
    let t = finish(&[
        entry(10, Some(1_800_000), Some(0.10)),
        entry(10, Some(1_800_000), Some(0.20)),
        entry(10, Some(3_600_000), None),
    ]);
    // Then: $0.30 over the priced hour; the unpriced hour does not dilute it.
    assert_eq!((t.samples, t.priced_samples), (3, 2));
    close(t.estimated_usd_per_active_hour, 0.30);
    close(Some(t.active_seconds), 7200.0);
}

#[test]
fn mode_labels_name_null_parts_and_pass_unknown_keys_through() {
    assert_eq!(
        mode_label(r#"["gpt-6","high",null]"#),
        "gpt-6 / high / -".to_owned()
    );
    assert_eq!(mode_label("not json"), "not json");
}

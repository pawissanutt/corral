use super::*;
use crate::model::daemon::detail_key;
use corral_types::UsageDay;

/// The header says what the partial totals cover and why the rest are missing; each group
/// states its own coverage with its reason (a run on a harness that reports no usage, a
/// request with tokens but no price); a profile with only silent runs still lists its runs
/// rather than vanishing; the day buckets add up; and quota is said to be unknown.
#[test]
fn coverage_is_stated_overall_and_per_group_with_the_reason_the_rest_is_missing() {
    let total = UsageTotal {
        requests: 4,
        input: 1200,
        output: 300,
        priced_requests: 2,
        usd: Some(0.5),
        ..UsageTotal::default()
    };
    let day = |day: &str, groups: Vec<UsageGroup>, covered, runs| UsageDay {
        day: day.into(),
        summary: UsageSummary {
            total: groups.iter().fold(UsageTotal::default(), |mut sum, group| {
                add(&mut sum, &group.total);
                sum
            }),
            groups,
            covered_runs: covered,
            total_runs: runs,
        },
    };
    let group = |key: &str, total: UsageTotal, covered, runs| UsageGroup {
        key: Some(key.into()),
        total,
        covered_runs: Some(covered),
        total_runs: Some(runs),
    };
    let series = UsageSeries {
        days: vec![
            day(
                "2025-11-14",
                vec![group("claude", UsageTotal::default(), 0, 1)],
                0,
                1,
            ),
            day(
                "2025-11-15",
                vec![
                    group("pi", total, 1, 1),
                    group("claude", UsageTotal::default(), 0, 1),
                ],
                1,
                2,
            ),
        ],
    };
    let mut feed = Feed {
        now: 1_763_200_000,
        ..Feed::default()
    };
    let state = State::default();
    let target = series_request(&state, &local(&feed));
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::UsageSeries(series)),
        },
    );
    let page = page(&state, &feed);
    assert!(page.head.contains("1 of 3 runs"), "{}", page.head);
    assert!(page.head.contains("reported usage on their start day"));
    assert!(
        page.coverage
            .unwrap()
            .reasons
            .iter()
            .any(|reason| reason.contains("no cost"))
    );
    let total = page.total.as_ref().unwrap();
    assert_eq!(total.covered, "1 of 3");
    let claude = page.rows.iter().find(|row| row.key == "claude").unwrap();
    assert_eq!(claude.covered, "0 of 2");
    assert_eq!(claude.requests, UNKNOWN);
    assert!(
        claude
            .reasons
            .as_deref()
            .unwrap()
            .contains("without matching usage in this group on the start day")
    );
    let pi = page.rows.iter().find(|row| row.key == "pi").unwrap();
    assert_eq!(pi.covered, "1 of 1");
    assert!(pi.reasons.as_deref().unwrap().contains("no reported cost"));
    assert!(!page.headers.contains(&"Run time"));
    let days = page.days.unwrap();
    assert_eq!(days.bars.len(), 30);
    assert_eq!(
        days.bars.iter().filter(|bar| bar.fraction == 1.0).count(),
        1
    );
}

/// A request left out of the estimate for incomplete token evidence is never described as
/// having no rate; only requests with complete tokens and no rate say that. Both the
/// missing-token count and the split survive the sum across days.
#[test]
fn missing_token_evidence_is_not_reported_as_a_missing_rate() {
    let group = |requests, estimated, missing_tokens| UsageGroup {
        key: Some("pi".into()),
        total: UsageTotal {
            requests,
            estimate_metadata_available: true,
            estimated_requests: estimated,
            estimate_missing_token_requests: missing_tokens,
            estimate_usd: (estimated > 0).then_some(1.0),
            ..UsageTotal::default()
        },
        covered_runs: Some(1),
        total_runs: Some(1),
    };
    let day = |day: &str, group: UsageGroup| UsageDay {
        day: day.into(),
        summary: UsageSummary {
            total: group.total.clone(),
            groups: vec![group],
            covered_runs: 1,
            total_runs: 1,
        },
    };
    // Day one: 1 estimated, 2 missing tokens. Day two: 1 estimated, 1 missing tokens, 1 no rate.
    let series = UsageSeries {
        days: vec![
            day("2025-11-14", group(3, 1, 2)),
            day("2025-11-15", group(3, 1, 1)),
        ],
    };
    let merged = merged(&series);
    assert_eq!(merged.total.estimate_missing_token_requests, 3);
    assert_eq!(missing_tokens(&merged.total), Some(3));
    assert_eq!(unrated(&merged.total), Some(1));
    let reasons = coverage(&merged).unwrap().reasons.join(" ");
    assert!(
            reasons.contains("covers 2 of the 6 requests; 3 had incomplete token evidence and 1 had no API rate known"),
            "{reasons}"
        );
    let (_, _, row) = group_coverage(&merged.groups[0]);
    let row = row.unwrap();
    assert!(
        row.contains("3 requests not in the estimate (incomplete token evidence)"),
        "{row}"
    );
    assert!(
        row.contains("1 request not in the estimate (no API rate known)"),
        "{row}"
    );
    // Only missing tokens: nothing claims a rate is missing.
    let only = group_coverage(&group(2, 0, 2)).2.unwrap();
    assert!(only.contains("incomplete token evidence"), "{only}");
    assert!(!only.contains("no API rate"), "{only}");
    let only = coverage(&UsageSummary {
        total: group(2, 0, 2).total,
        ..UsageSummary::default()
    })
    .unwrap()
    .reasons
    .join(" ");
    assert!(!only.contains("no API rate"), "{only}");
}

/// The estimate is its own column: it sums across days without ever joining the
/// reported cost, stays unknown where nothing could be priced, and its coverage names the
/// requests without a rate and the start-day limitation of run coverage.
#[test]
fn estimate_is_kept_apart_from_reported_cost_and_its_coverage_is_stated() {
    let pi = |requests, priced, usd, estimated, estimate| UsageGroup {
        key: Some("pi".into()),
        total: UsageTotal {
            requests,
            priced_requests: priced,
            usd,
            estimate_metadata_available: true,
            estimated_requests: estimated,
            estimate_usd: estimate,
            ..UsageTotal::default()
        },
        covered_runs: Some(1),
        total_runs: Some(2),
    };
    let codex = UsageGroup {
        key: Some("codex".into()),
        total: UsageTotal {
            requests: 1,
            estimate_metadata_available: true,
            ..UsageTotal::default()
        },
        covered_runs: Some(1),
        total_runs: Some(1),
    };
    let day = |day: &str, groups: Vec<UsageGroup>| UsageDay {
        day: day.into(),
        summary: UsageSummary {
            total: groups.iter().fold(UsageTotal::default(), |mut sum, group| {
                add(&mut sum, &group.total);
                sum
            }),
            covered_runs: groups.iter().filter_map(|g| g.covered_runs).sum(),
            total_runs: groups.iter().filter_map(|g| g.total_runs).sum(),
            groups,
        },
    };
    let series = UsageSeries {
        days: vec![
            day("2025-11-14", vec![pi(2, 2, Some(0.5), 2, Some(1.25))]),
            day("2025-11-15", vec![pi(2, 0, None, 1, Some(0.75)), codex]),
        ],
    };
    let mut feed = Feed {
        now: 1_763_200_000,
        ..Feed::default()
    };
    let state = State::default();
    let target = series_request(&state, &local(&feed));
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::UsageSeries(series)),
        },
    );
    let page = page(&state, &feed);
    assert_eq!(page.headers[4..], ["Reported", "API est."]);
    assert!(page.estimate_note.contains("not a charge"));
    let total = page.total.as_ref().unwrap();
    assert_eq!(total.cost, "$0.50");
    assert_eq!(total.estimate, "$2.00");
    assert_eq!(
        estimate(&UsageTotal {
            estimate_usd: Some(0.002),
            ..UsageTotal::default()
        }),
        "<$0.01"
    );
    assert_eq!(
        estimate(&UsageTotal {
            estimate_usd: Some(0.0),
            ..UsageTotal::default()
        }),
        "$0.00"
    );
    let pi = page.rows.iter().find(|row| row.key == "pi").unwrap();
    assert_eq!((pi.cost.as_str(), pi.estimate.as_str()), ("$0.50", "$2.00"));
    assert!(
        pi.reasons
            .as_deref()
            .unwrap()
            .contains("not in the estimate")
    );
    let codex = page.rows.iter().find(|row| row.key == "codex").unwrap();
    assert_eq!(
        (codex.cost.as_str(), codex.estimate.as_str()),
        (UNKNOWN, UNKNOWN)
    );
    let reasons = page.coverage.unwrap().reasons.join(" ");
    assert!(reasons.contains("covers 3 of the 5 requests"), "{reasons}");
    assert!(
        reasons.contains("later requests are still included"),
        "{reasons}"
    );
}

/// Input and output are only a number where requests carried evidence for them: absent
/// evidence is a dash (not 0), incomplete evidence is a lower bound with its reason, an
/// explicit zero is 0, and a daemon that predates the evidence counts is never shown as
/// complete. The day and group sums keep this.
#[test]
fn token_totals_follow_their_evidence_not_their_sum() {
    let group = |key: &str, requests, input, output, seen: (Option<u64>, Option<u64>)| UsageGroup {
        key: Some(key.into()),
        total: UsageTotal {
            requests,
            input,
            output,
            input_requests: seen.0,
            output_requests: seen.1,
            ..UsageTotal::default()
        },
        covered_runs: Some(1),
        total_runs: Some(1),
    };
    let day = |day: &str, groups: Vec<UsageGroup>| UsageDay {
        day: day.into(),
        summary: UsageSummary {
            total: groups.iter().fold(UsageTotal::default(), |mut sum, group| {
                add(&mut sum, &group.total);
                sum
            }),
            covered_runs: groups.len() as u64,
            total_runs: groups.len() as u64,
            groups,
        },
    };
    let both = |n| (Some(n), Some(n));
    let series = UsageSeries {
        days: vec![
            day(
                "2025-11-14",
                vec![
                    group("zero", 2, 0, 0, both(2)),
                    group("silent", 1, 0, 0, both(0)),
                    group("mixed", 3, 300, 10, (Some(1), Some(3))),
                    group("legacy", 2, 500, 0, (None, None)),
                    group("legacy-zero", 1, 0, 0, (None, None)),
                    group("split", 2, 100, 0, both(2)),
                ],
            ),
            day("2025-11-15", vec![group("split", 1, 0, 0, both(0))]),
        ],
    };
    let summary = merged(&series);
    let rows = rows(UsageBy::Profile, &summary, &Feed::default());
    let row = |key: &str| rows.iter().find(|row| row.key == key).unwrap();
    let figures = |key: &str| (row(key).input.clone(), row(key).output.clone());
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(figures("zero"), pair("0", "0"));
    assert_eq!(figures("silent"), pair(UNKNOWN, UNKNOWN));
    assert_eq!(figures("mixed"), pair("≥300", "10"));
    assert_eq!(figures("legacy"), pair("≥500", UNKNOWN));
    assert_eq!(figures("legacy-zero"), pair(UNKNOWN, UNKNOWN));
    // Two of three requests across the days reported: the sum is a lower bound.
    assert_eq!(figures("split"), pair("≥100", "≥0"));
    let reasons = |key: &str| row(key).reasons.clone().unwrap_or_default();
    assert!(!reasons("zero").contains("tokens"), "{}", reasons("zero"));
    assert!(
        reasons("silent").contains("no request reported input or output tokens"),
        "{}",
        reasons("silent")
    );
    assert!(
        reasons("mixed").contains("input tokens reported on 1 of 3 requests"),
        "{}",
        reasons("mixed")
    );
    assert!(
        reasons("legacy").contains("input tokens: how many requests reported them is unknown"),
        "{}",
        reasons("legacy")
    );
    assert!(
        reasons("split").contains("input tokens reported on 2 of 3 requests"),
        "{}",
        reasons("split")
    );
    // A legacy part makes the period's total a lower bound, said once in the page's notes.
    let total = total_row(&summary);
    assert_eq!(total.input, "≥900");
    assert_eq!(total.output, "≥10");
    let notes = coverage(&summary).unwrap().reasons.join(" ");
    assert!(
        notes.contains("Input and output tokens are incomplete"),
        "{notes}"
    );
    // These fixtures predate the estimate metadata: every group and the period say their
    // estimate coverage is unknown, and none claims a count of covered, token-less or
    // rate-less requests.
    for key in ["zero", "silent", "mixed", "legacy", "legacy-zero", "split"] {
        let reasons = reasons(key);
        assert!(reasons.contains("estimate coverage unknown"), "{reasons}");
        assert!(!reasons.contains("not in the estimate"), "{reasons}");
        assert_eq!(row(key).estimate, UNKNOWN);
    }
    assert_eq!(total.estimate, UNKNOWN);
    assert!(notes.contains("estimate coverage is unknown"), "{notes}");
    assert!(!notes.contains("covers"), "{notes}");
    assert!(!notes.contains("no API rate"), "{notes}");
    // The day strip says the same.
    assert_eq!(used_words(&group("zero", 2, 0, 0, both(2)).total), "0");
    assert_eq!(
        used_words(&group("silent", 1, 0, 0, both(0)).total),
        UNKNOWN
    );
    assert_eq!(used_words(&UsageTotal::default()), UNKNOWN);
    assert_eq!(
        used_words(&group("mixed", 3, 300, 10, (Some(1), Some(3))).total),
        "≥310"
    );
}

/// The day strip's note claims no more than the evidence: an exact busiest day only when
/// every reporting day is complete, a lower bound floored (1,051 is `≥1.0k`, never
/// `≥1.1k`), and neither explicit zeros nor a legacy zero read as "nothing reported".
#[test]
fn day_strip_note_follows_token_evidence() {
    let strip = |requests, input, seen: (Option<u64>, Option<u64>)| {
        let series = UsageSeries {
            days: vec![UsageDay {
                day: "2025-11-14".into(),
                summary: UsageSummary {
                    total: UsageTotal {
                        requests,
                        input,
                        input_requests: seen.0,
                        output_requests: seen.1,
                        ..UsageTotal::default()
                    },
                    ..UsageSummary::default()
                },
            }],
        };
        let day = jiff::civil::date(2025, 11, 14);
        day_strip(
            Days {
                first: day,
                last: day,
            },
            &series,
        )
    };
    let exact = strip(1, 1_051, (Some(1), Some(1))).note;
    assert!(
        exact.starts_with("busiest") && exact.contains("14 Nov"),
        "{exact}"
    );
    let partial_strip = strip(2, 1_051, (Some(1), Some(1)));
    assert!(partial_strip.spoken.contains("busiest may differ"));
    let partial = partial_strip.note;
    assert!(
        !partial.contains("busiest:") && !partial.starts_with("busiest"),
        "{partial}"
    );
    assert!(
        partial.contains("≥1.0k") && !partial.contains("1.1k"),
        "{partial}"
    );
    let zero = strip(2, 0, (Some(2), Some(2))).note;
    assert_eq!(zero, "every token count reported was zero");
    let legacy_zero = strip(1, 0, (None, None));
    assert_eq!(legacy_zero.note, "token reporting unknown");
    assert!(
        !legacy_zero.note.contains("no day reported"),
        "{}",
        legacy_zero.note
    );
    assert!(
        legacy_zero.spoken.contains("unknown"),
        "{}",
        legacy_zero.spoken
    );
    assert_eq!(
        strip(1, 0, (Some(0), Some(0))).note,
        "no day reported any tokens"
    );
    assert_eq!(
        strip(0, 0, (Some(0), Some(0))).note,
        "no day reported any tokens"
    );
    assert_eq!(figure_of(Evidence::Partial(None), 1_051), "≥1.0k");
    assert_eq!(floor_tokens(999_999), "999.9k");
    assert_eq!(floor_tokens(1_999_999), "1.9M");
}

/// A day from before the estimate metadata merged with a day that has it leaves the period and
/// the group unknown: the new day's counts are not presented as the whole coverage, and its
/// estimate is only a lower bound. A day without requests changes nothing.
#[test]
fn legacy_and_new_days_merge_to_unknown_estimate_coverage() {
    let group = |requests, available, estimated| UsageGroup {
        key: Some("pi".into()),
        total: UsageTotal {
            requests,
            estimate_metadata_available: available,
            estimated_requests: estimated,
            estimate_missing_token_requests: estimated.min(1),
            estimate_usd: (estimated > 0).then_some(12.75),
            ..UsageTotal::default()
        },
        covered_runs: Some(1),
        total_runs: Some(1),
    };
    let day = |day: &str, group: UsageGroup| UsageDay {
        day: day.into(),
        summary: UsageSummary {
            total: group.total.clone(),
            groups: vec![group],
            covered_runs: 1,
            total_runs: 1,
        },
    };
    let series = |days: Vec<UsageDay>| merged(&UsageSeries { days });
    let reasons = |summary: &UsageSummary| coverage(summary).unwrap().reasons.join(" ");
    for order in [[(2, false, 0), (3, true, 2)], [(3, true, 2), (2, false, 0)]] {
        let [first, second] = order;
        let summary = series(vec![
            day("2025-11-14", group(first.0, first.1, first.2)),
            day("2025-11-15", group(second.0, second.1, second.2)),
        ]);
        assert!(!summary.total.estimate_metadata_available);
        assert_eq!(missing_tokens(&summary.total), None);
        assert_eq!(unrated(&summary.total), None);
        assert_eq!(estimate(&summary.total), "≥$12");
        let notes = reasons(&summary);
        assert!(notes.contains("estimate coverage is unknown"), "{notes}");
        assert!(!notes.contains("covers"), "{notes}");
        let (_, _, row) = group_coverage(&summary.groups[0]);
        let row = row.unwrap();
        assert!(row.contains("estimate coverage unknown"), "{row}");
        assert!(!row.contains("not in the estimate"), "{row}");
    }
    // A day with no requests is neutral, in either order: known stays known.
    for days in [
        vec![
            day("2025-11-14", group(0, false, 0)),
            day("2025-11-15", group(3, true, 2)),
        ],
        vec![
            day("2025-11-14", group(3, true, 2)),
            day("2025-11-15", group(0, false, 0)),
        ],
    ] {
        let summary = series(days);
        assert!(summary.total.estimate_metadata_available);
        assert_eq!(estimate(&summary.total), "$13");
        assert_eq!(missing_tokens(&summary.total), Some(1));
        assert!(
            reasons(&summary).contains("covers 2 of the 3 requests"),
            "{}",
            reasons(&summary)
        );
    }
}

/// A legacy part merged with an explicit-zero part is unknown, not "no request reported".
#[test]
fn merged_legacy_and_explicit_zero_is_not_none_reported() {
    let part = |seen: Option<u64>| UsageTotal {
        requests: 1,
        input_requests: seen,
        output_requests: seen,
        ..UsageTotal::default()
    };
    let mut sum = UsageTotal::default();
    add(&mut sum, &part(None));
    add(&mut sum, &part(Some(1)));
    assert_eq!(input_evidence(&sum), Evidence::Unknown);
    assert_eq!(figure_of(input_evidence(&sum), sum.input), UNKNOWN);
    let reasons = token_reasons(&sum).join("; ");
    assert!(!reasons.contains("no request reported"), "{reasons}");
    assert!(reasons.contains("unknown"), "{reasons}");
}

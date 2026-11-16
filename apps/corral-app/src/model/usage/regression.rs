use super::*;
use corral_protocol::detail::{ProfileUsage, UsageCompleteness, UsageDay, UsageQuota};
use corral_protocol::rows::LimitBasis;

fn coverage() -> UsageCompleteness {
    UsageCompleteness {
        scanned_rows: 5,
        ..super::super::completeness::complete()
    }
}

fn row(connection: &str, profile: Option<&str>, cost: Option<f64>) -> UsageRow {
    UsageRow {
        id: format!("{connection}/{profile:?}"),
        connection: connection.into(),
        profile: profile.map(str::to_owned),
        window: None,
        used: None,
        limit: None,
        limit_basis: LimitBasis::None,
        resets_at: None,
        today_usd: cost,
        week_usd: cost,
        cost_basis: Some(CostBasis::Reported),
        today_completeness: coverage(),
        week_completeness: coverage(),
        quotas: vec![],
        rev: 1,
    }
}

fn input<'a>(
    rows: Vec<&'a UsageRow>,
    series: Option<&'a UsageSeries>,
    tz: &'a TimeZone,
) -> Inputs<'a> {
    Inputs {
        rows,
        series,
        rows_error: None,
        rows_loading: false,
        series_error: None,
        series_stale: false,
        agents: vec![],
        assistants: vec![],
        period: Period::Today,
        now: 1_700_000_000,
        tz,
    }
}

fn usage(connection: &str, profile: &str, cost: Option<f64>) -> ProfileUsage {
    ProfileUsage {
        connection: connection.into(),
        profile: profile.into(),
        cost_usd: cost,
        cost_basis: Some(CostBasis::Reported),
        tokens: Some(1),
        completeness: coverage(),
    }
}

/// Every card, profile line and daily bar once repeated the same partial scan with raw ISO
/// stamps; the scan is said once for the page in calendar days, while each profile keeps its own
/// unpriced entries and no total loses its qualification.
#[test]
fn partial_coverage_is_said_once_in_days_and_rows_keep_their_unpriced_entries() {
    let known = row("c", Some("known"), Some(2.0));
    let mut unknown = row("c", Some("unknown"), None);
    let partial = UsageCompleteness {
        partial: true,
        reason: Some("row_limit".into()),
        unpriced_entries: 3,
        scanned_from: Some("2023-10-25T05:28:18.427Z".into()),
        scanned_to: Some("2023-11-14T01:31:49.207Z".into()),
        ..coverage()
    };
    unknown.today_completeness = partial.clone();
    unknown.week_completeness = partial.clone();
    let mut unpriced = usage("c", "unknown", None);
    unpriced.completeness = partial.clone();
    let series = UsageSeries {
        providers: vec![],
        days: vec![UsageDay {
            day: "2026-11-15".into(),
            by_profile: vec![usage("c", "known", Some(2.0)), unpriced],
        }],
        completeness: partial,
    };
    let tz = TimeZone::UTC;
    let mut inputs = input(vec![&known, &unknown], Some(&series), &tz);
    for period in [Period::Today, Period::Week, Period::Month] {
        inputs.period = period;
        let page = page(&inputs);
        let said = page.coverage.as_deref().unwrap();
        assert!(!said.contains("T05:28") && !said.contains(".427Z"), "{said}");
        assert!(page.providers.iter().all(|card| card.note.is_none()));
        assert!(page.groups[0].rows[1].gaps.is_some());
        assert_eq!(page.groups[0].rows[0].gaps, None);
    }
}

#[test]
fn monthly_totals_and_series_legend_preserve_connection_and_unknown_profile_keys() {
    let rows = [
        row("a", Some("p"), None),
        row("b", Some("p"), None),
        row("a", None, None),
    ];
    let series = UsageSeries {
        providers: vec![],
        days: vec![UsageDay {
            day: "2026-11-15".into(),
            by_profile: vec![
                usage("a", "p", Some(2.0)),
                usage("b", "p", Some(3.0)),
                usage("a", "", Some(7.0)),
            ],
        }],
        completeness: coverage(),
    };
    let tz = TimeZone::UTC;
    let mut inputs = input(rows.iter().collect(), Some(&series), &tz);
    inputs.period = Period::Month;
    assert_eq!(row_cost(&rows[0], &inputs).total, 2.0);
    assert_eq!(row_cost(&rows[1], &inputs).total, 3.0);
    assert_eq!(row_cost(&rows[2], &inputs).total, 7.0);
    assert_eq!(page(&inputs).costs.legend.len(), 3);
}

#[test]
fn failed_or_partial_usage_never_claims_an_empty_store_and_reports_one_cause() {
    let tz = TimeZone::UTC;
    let mut inputs = input(vec![], None, &tz);
    inputs.rows_error =
        Some("app topic unavailable: app topic unavailable: unavailable: scan failed");
    inputs.series_error = Some("unavailable: scan failed");
    let failed = page(&inputs);
    assert_eq!(failed.notices.len(), 1);
    assert_eq!(failed.notices[0].matches("scan failed").count(), 1);
    assert!(failed.providers_empty.is_none());
    let stored_nothing = UsageSeries {
        providers: vec![],
        days: vec![],
        completeness: coverage(),
    };
    let empty_store = page(&input(vec![], Some(&stored_nothing), &tz));
    assert!(empty_store.costs.empty.is_some());
    assert_ne!(failed.costs.empty, empty_store.costs.empty);
    let series = UsageSeries {
        providers: vec![],
        days: vec![],
        completeness: Default::default(),
    };
    inputs.series = Some(&series);
    inputs.rows_error = None;
    inputs.series_error = None;
    assert!(page(&inputs).providers_empty.is_none());
}

#[test]
fn retained_series_errors_qualify_monthly_costs_as_stale() {
    let row = row("a", Some("p"), Some(1.0));
    let series = UsageSeries {
        providers: vec![],
        days: vec![UsageDay {
            day: "2026-11-15".into(),
            by_profile: vec![usage("a", "p", Some(1.0))],
        }],
        completeness: coverage(),
    };
    let tz = TimeZone::UTC;
    let mut inputs = input(vec![&row], Some(&series), &tz);
    inputs.period = Period::Month;
    inputs.series_error = Some("refresh failed");
    let page = page(&inputs);
    // The series' own coverage is whole, so only its staleness can qualify the page.
    assert!(page.coverage.is_some());
    assert!(page.costs.note.is_some());
    assert_eq!(page.notices.len(), 1);
}

#[test]
fn exhaustion_observations_do_not_fabricate_numerical_quotas() {
    let mut row = row("a", Some("p"), Some(1.0));
    row.quotas.push(UsageQuota {
        source: "availability_exhaustion".into(),
        scope: Some("account".into()),
        window: None,
        used: None,
        limit: None,
        headroom: None,
        resets_at: Some("2023-11-14T23:00:00Z".into()),
        observed_at: Some("2023-11-14T22:00:00Z".into()),
        exhausted: Some(true),
        completeness: coverage(),
    });
    let tz = TimeZone::UTC;
    let page = page(&input(vec![&row], None, &tz));
    let provider = &page.providers[0];
    assert_eq!(provider.used, None);
    assert!(provider.exhausted);
    // Only the reset clock carries digits; no use, limit or headroom figure is made up.
    assert!(
        provider
            .quota_line
            .replace("23:00", "")
            .chars()
            .all(|c| !c.is_ascii_digit()),
        "{}",
        provider.quota_line
    );
    assert!(
        !provider.headroom.chars().any(|c| c.is_ascii_digit()),
        "{}",
        provider.headroom
    );
}

/// The card header once required an account-scoped fact and said "window unknown", "quota
/// unknown" and "reset unknown" over details listing that very window, use and reset; float
/// headroom also leaked as `66.00000000000001`.
#[test]
fn provider_header_reads_the_quota_its_card_lists() {
    let mut row = row("a", Some("p"), Some(1.0));
    row.quotas.push(UsageQuota {
        source: "provider_api".into(),
        scope: Some("profile".into()),
        window: Some("5 h".into()),
        used: Some(34.0),
        limit: Some(100.0),
        headroom: Some(0.66 * 100.0),
        resets_at: Some("2023-11-14T23:00:00Z".into()),
        observed_at: Some("2023-11-14T22:00:00Z".into()),
        exhausted: Some(false),
        completeness: coverage(),
    });
    let tz = TimeZone::UTC;
    let page = page(&input(vec![&row], None, &tz));
    let provider = &page.providers[0];
    assert_eq!(provider.used, Some(0.34));
    let aside = provider.aside.clone().unwrap_or_default();
    for said in [&aside, &provider.quota_line, &provider.headroom] {
        assert!(!said.contains(UNKNOWN) && !said.contains("66.0"), "{said}");
    }
    assert!(provider.headroom.contains("66"));
}

/// Usage recorded without a connection made a card with no title over a giant "unknown".
#[test]
fn unattributed_usage_has_a_title_and_follows_named_providers() {
    let rows = [row("", Some("p"), None), row("b", Some("p"), Some(1.0))];
    let tz = TimeZone::UTC;
    let page = page(&input(rows.iter().collect(), None, &tz));
    let labels: Vec<&str> = page.providers.iter().map(|card| card.label.as_str()).collect();
    assert_eq!(labels, ["b", UNATTRIBUTED]);
    assert!(page.providers.iter().all(|card| !card.id.is_empty()));
}

/// corrald lists a profile with no quota source as an observation holding nothing; every such
/// profile became a provenance line of "unknown" fields under a "partial" warning.
#[test]
fn empty_quota_observations_are_no_quota_rather_than_lines_of_unknowns() {
    let mut row = row("a", Some("p"), Some(1.0));
    row.quotas.push(UsageQuota {
        source: "unknown".into(),
        scope: None,
        window: None,
        used: None,
        limit: None,
        headroom: None,
        resets_at: None,
        observed_at: None,
        exhausted: None,
        completeness: Default::default(),
    });
    let tz = TimeZone::UTC;
    let page = page(&input(vec![&row], None, &tz));
    let provider = &page.providers[0];
    assert!(!provider.quota_known);
    assert!(provider.quotas.is_empty());
}

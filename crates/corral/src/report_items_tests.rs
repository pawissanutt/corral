use super::*;

#[test]
fn project_filter_excludes_projectless_usage_and_reconciles_totals_and_coverage() {
    let mut report = CostReport {
        rows: vec![
            CostRow {
                class: CostClass::Item,
                project: Some("a".into()),
                task: Some("T-001".into()),
                requests: 2,
                input: 10,
                cache_read: 20,
                cache_write: 30,
                output: 40,
                // One priced request and one unpriced request share this item row.
                unpriced_tokens: 70,
                usd: Some(0.5),
                ..CostRow::default()
            },
            CostRow {
                class: CostClass::Item,
                project: Some("b".into()),
                requests: 1,
                input: 100,
                usd: Some(2.0),
                ..CostRow::default()
            },
            CostRow {
                class: CostClass::Overhead,
                requests: 1,
                usd: Some(1.0),
                ..CostRow::default()
            },
            CostRow {
                requests: 1,
                input: 1000,
                unpriced_tokens: 1000,
                ..CostRow::default()
            },
        ],
        coverage: corral_ops::CostCoverage {
            attributed_requests: 4,
            total_requests: 5,
            legacy: 1,
            dropped: 3,
        },
        total: corral_core::Usage {
            input: 1110,
            cache_read: 20,
            cache_write: 30,
            output: 40,
            unpriced_tokens: 1070,
            estimated_usd: Some(3.5),
            ..corral_core::Usage::default()
        },
    };
    let unfiltered = serde_json::to_value(&report).unwrap();
    assert_eq!(unfiltered["rows"][0]["usd"], 0.5);
    assert_eq!(unfiltered["rows"][0]["unpriced_tokens"], 70);
    assert_eq!(unfiltered["total"]["unpriced_tokens"], 1070);
    filter_project(&mut report, "a");
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].task.as_deref(), Some("T-001"));
    assert_eq!(report.total.input, 10);
    assert_eq!(report.total.cache_read, 20);
    assert_eq!(report.total.cache_write, 30);
    assert_eq!(report.total.output, 40);
    assert_eq!(report.total.estimated_usd, Some(0.5));
    assert_eq!(report.total.unpriced_tokens, 70);
    assert_eq!(
        report.total.unpriced_tokens,
        report
            .rows
            .iter()
            .map(|row| row.unpriced_tokens)
            .sum::<u64>()
    );
    assert_eq!(report.coverage.total_requests, 2);
    assert_eq!(report.coverage.attributed_requests, 2);
    assert_eq!(report.coverage.legacy, 0);
    let json = serde_json::to_value(Output {
        report: &report,
        filtered_project: Some("a"),
        attribution_drops_scope: Some("all_projects"),
    })
    .unwrap();
    assert_eq!(json["filtered_project"], "a");
    assert_eq!(json["attribution_drops_scope"], "all_projects");
    assert_eq!(json["coverage"]["dropped"], 3);
    assert_eq!(json["rows"][0]["usd"], 0.5);
    assert_eq!(json["rows"][0]["unpriced_tokens"], 70);
    assert_eq!(json["total"]["unpriced_tokens"], 70);
}

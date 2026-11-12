use anyhow::{Context, Result, ensure};
use corral_core::agent::{pm_cost::CostClass, pm_state};
use corral_ops::{CostReport, CostRow, CostWindow, Ctx};
use serde::Serialize;

use super::ReportArgs;

#[derive(Serialize)]
struct Output<'a> {
    #[serde(flatten)]
    report: &'a CostReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    filtered_project: Option<&'a str>,
    /// Retention drops cannot be assigned to a project after their records are gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    attribution_drops_scope: Option<&'static str>,
}

pub(super) fn run(ctx: &Ctx, args: &ReportArgs) -> Result<()> {
    let now = corral_core::clock::now();
    let since = now.checked_sub(corral_core::parse_age(&args.since).context("--since")?)?;
    let until = args.until.unwrap_or(now);
    ensure!(since <= until, "report window starts after its end");
    if let Some(project) = &args.project {
        ensure!(
            ctx.store.config.projects.contains_key(project),
            "unknown project {project}"
        );
    }
    // An initialized store need not have hosted a PM yet. Do not launch or refresh one.
    let mut report = if pm_state::load(&ctx.store)?.is_some() {
        corral_ops::pm_cost_by_item(
            ctx,
            CostWindow {
                since: Some(since),
                until: Some(until),
            },
        )?
    } else {
        CostReport {
            rows: [CostClass::Overhead, CostClass::Unallocated]
                .into_iter()
                .map(|class| CostRow {
                    class,
                    ..CostRow::default()
                })
                .collect(),
            ..CostReport::default()
        }
    };
    if let Some(project) = args.project.as_deref() {
        filter_project(&mut report, project);
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Output {
                report: &report,
                filtered_project: args.project.as_deref(),
                attribution_drops_scope: args.project.as_ref().map(|_| "all_projects"),
            })?
        );
    } else {
        print_table(&mut report, args.project.as_deref());
    }
    Ok(())
}

fn filter_project(report: &mut CostReport, project: &str) {
    report
        .rows
        .retain(|row| row.project.as_deref() == Some(project));
    // The library's aggregate entries have no item identity. Rebuild only the evidence
    // available in the retained rows, rather than leaking other projects' total usage.
    report.total = corral_core::Usage::default();
    report.coverage.total_requests = 0;
    report.coverage.attributed_requests = 0;
    // Requests without structural turn markers are always projectless/unallocated.
    report.coverage.legacy = 0;
    for row in &report.rows {
        report.coverage.total_requests += row.requests;
        if row.class != CostClass::Unallocated {
            report.coverage.attributed_requests += row.requests;
        }
        report.total.input = report.total.input.saturating_add(row.input);
        report.total.cache_read = report.total.cache_read.saturating_add(row.cache_read);
        report.total.cache_write = report.total.cache_write.saturating_add(row.cache_write);
        report.total.output = report.total.output.saturating_add(row.output);
        if let Some(usd) = row.usd {
            *report.total.estimated_usd.get_or_insert(0.0) += usd;
        } else {
            report.total.unpriced_tokens = report.total.unpriced_tokens.saturating_add(
                row.input
                    .saturating_add(row.cache_read)
                    .saturating_add(row.cache_write)
                    .saturating_add(row.output),
            );
        }
    }
    if report.total.estimated_usd.is_some() {
        report.total.cost_basis = Some(corral_core::CostBasis::Estimated);
    }
}

fn class_name(class: CostClass) -> &'static str {
    match class {
        CostClass::Item => "item",
        CostClass::Shared => "shared",
        CostClass::Overhead => "overhead",
        CostClass::Unallocated => "unallocated",
    }
}

fn print_table(report: &mut CostReport, project: Option<&str>) {
    println!("API-equivalent estimates for reporting, not limits");
    if let Some(project) = project {
        println!(
            "Filtered project: {project}; totals and request coverage exclude other projects and projectless usage"
        );
    }
    println!(
        "{:<11} {:<16} {:<24} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "CLASS",
        "PROJECT",
        "TASK / ITEM",
        "REQUESTS",
        "INPUT",
        "CACHE READ",
        "CACHE WRITE",
        "OUTPUT",
        "USD"
    );
    report.rows.sort_by(|a, b| {
        b.usd
            .partial_cmp(&a.usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for row in &report.rows {
        print_row(class_name(row.class), row);
    }
    print_row(
        "total",
        &CostRow {
            requests: report.coverage.total_requests,
            input: report.total.input,
            cache_read: report.total.cache_read,
            cache_write: report.total.cache_write,
            output: report.total.output,
            usd: report.total.estimated_usd,
            ..CostRow::default()
        },
    );
    println!(
        "attributed {} of {} requests; {} legacy requests without turn markers; {} attribution records dropped{}",
        report.coverage.attributed_requests,
        report.coverage.total_requests,
        report.coverage.legacy,
        report.coverage.dropped,
        if project.is_some() {
            " (all projects)"
        } else {
            ""
        },
    );
}

fn print_row(class: &str, row: &CostRow) {
    let usd = row.usd.map_or_else(
        || {
            if row.requests == 0 {
                "0.0000".into()
            } else {
                "-".into()
            }
        },
        |usd| format!("{usd:.4}"),
    );
    println!(
        "{class:<11} {:<16} {:<24} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12}",
        row.project.as_deref().unwrap_or("-"),
        row.task
            .as_deref()
            .or(row.item_key.as_deref())
            .unwrap_or("-"),
        row.requests,
        row.input,
        row.cache_read,
        row.cache_write,
        row.output,
        usd,
    );
}

#[cfg(test)]
#[path = "report_items_tests.rs"]
mod tests;

use crate::cmds::route::run_profile;
use anyhow::{Context, Result, bail};
use corral_ops::throughput::{
    RATE_NOTE, fixed, mode_label, mode_throughput, throughput_report_html,
};
use corral_ops::{Ctx, StatsBy};

pub fn show_run_usage(run: &corral_core::Run, indent: &str) {
    let summary = run.usage.as_ref().map(|u| u.summary()).unwrap_or_else(|| {
        format!(
            "unknown ({})",
            run.usage_note.as_deref().unwrap_or("no usage")
        )
    });
    println!("{indent}cost: {summary}");
    for reviewer in &run.review_runs {
        println!(
            "{indent}reviewer run {} [{}]",
            reviewer.id,
            run_profile(reviewer)
        );
        show_run_usage(reviewer, &format!("{indent}    "));
    }
}

pub fn show_total_usage(task: &corral_core::Task) {
    let summary = task
        .total_usage()
        .map(|u| u.summary())
        .unwrap_or_else(|| "unknown".into());
    println!("total usage: {summary}");
}

/// The cutoff `--since <age>` (N h/d/w, e.g. 30d) names: that long before now.
pub fn since_cutoff(since: &str) -> Result<jiff::Timestamp> {
    let age = corral_core::parse_age(since).context("--since")?;
    jiff::Timestamp::now()
        .checked_sub(age)
        .context("--since is out of range")
}

/// A `--since`/`--until` bound for `--gates`: an RFC 3339 timestamp, or for `--since` an age.
fn gate_bound(value: &str, flag: &str, age: bool) -> Result<jiff::Timestamp> {
    match value.parse::<jiff::Timestamp>() {
        Ok(at) => Ok(at),
        Err(_) if age => since_cutoff(value),
        Err(e) => Err(e).with_context(|| format!("{flag} must be an RFC 3339 timestamp")),
    }
}

pub fn run_gates(ctx: &Ctx, since: Option<&str>, until: Option<&str>, json: bool) -> Result<()> {
    use corral_ops::analytics::gate_metrics::{GateFigures, gate_stats};
    let since = since.map(|s| gate_bound(s, "--since", true)).transpose()?;
    let until = until.map(|u| gate_bound(u, "--until", false)).transpose()?;
    let report = gate_stats(&ctx.store, since, until)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let opt = |v: Option<f64>| v.map_or_else(|| "-".into(), |v| format!("{v:.2}"));
    let line = |label: &str, f: &GateFigures| {
        let class = |name: &str| f.classes.get(name).copied().unwrap_or(0);
        println!(
            "{label:<48} {:>6} {:>6} {:>6} {:>6} {:>7.1} {:>7} {:>7} {:>7} {:>7} {:>6} {:>6}",
            f.gates_started,
            class("pass"),
            class("command_failed"),
            class("stale_result"),
            f.gate_hours,
            f.landed_tasks,
            opt(f.gates_per_landed_task),
            opt(f.gate_hours_per_landed_task),
            opt(f.gates_per_review_round),
            f.behind.behind_at_start.gates,
            f.worker_receipts_reused_by_autopilot,
        );
    };
    println!(
        "{:<48} {:>6} {:>6} {:>6} {:>6} {:>7} {:>7} {:>7} {:>7} {:>7} {:>6} {:>6}",
        "COHORT / TIER / DESTINATION",
        "GATES",
        "PASS",
        "FAIL",
        "STALE",
        "HOURS",
        "LANDED",
        "G/LAND",
        "H/LAND",
        "G/ROUND",
        "BEHIND",
        "REUSED"
    );
    for group in &report.groups {
        let cohort = serde_json::to_value(group.cohort)?;
        let label = format!(
            "{} / {} / {}",
            cohort.as_str().unwrap_or("?"),
            group.tier,
            group.destination
        );
        line(&label, &group.figures);
    }
    line("total", &report.totals);
    println!("unmatched gate events: {}", report.gate_events_unmatched);
    for carry in &report.carries {
        let cohort = serde_json::to_value(carry.cohort)?;
        let methods = |m: &std::collections::BTreeMap<String, u32>| {
            m.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        println!(
            "carries {} / {} -> {} / {}: gate [{}] verdict [{}] evidence [{}]",
            cohort.as_str().unwrap_or("?"),
            carry.from_tier,
            carry.to_tier,
            carry.destination,
            methods(&carry.carries.gate),
            methods(&carry.carries.verdict),
            methods(&carry.carries.evidence)
        );
    }
    if !report.totals.full_step_seconds.is_empty() {
        println!("\nfull-gate step seconds (samples, median, total):");
        for (command, step) in &report.totals.full_step_seconds {
            println!(
                "  {:>5} {:>8} {:>10.1}  {command}",
                step.samples,
                opt(step.median_seconds),
                step.total_seconds
            );
        }
    }
    Ok(())
}

pub fn run(
    ctx: &Ctx,
    by: &str,
    since: Option<&str>,
    json: bool,
    html: bool,
    out: Option<&std::path::Path>,
) -> Result<()> {
    let by_kind = match by {
        "profile" => StatsBy::Profile,
        "model" => StatsBy::Model,
        "kind" => StatsBy::Kind,
        "project" => StatsBy::Project,
        "task" => StatsBy::Task,
        "size" => StatsBy::Size,
        "mode" => StatsBy::Mode,
        "day" => StatsBy::Day,
        _ => bail!("unknown stats grouping {by}"),
    };
    let cutoff = since.map(since_cutoff).transpose()?;
    if html {
        return write_html_report(ctx, by_kind, cutoff, out);
    }
    let rows = corral_ops::stats(&ctx.store, by_kind, cutoff)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        let tokens = rows.iter().any(|r| r.total_tokens.is_some());
        let costs = rows.iter().any(|r| r.total_cost_usd.is_some());
        let estimates = rows.iter().any(|r| r.total_estimated_usd.is_some());
        let unpriced = rows
            .iter()
            .any(|r| r.unpriced_tokens.is_some_and(|n| n > 0));
        print!(
            "{by:<20} {:>5} {:>12} {:>11} {:>10} {:>8} {:>14} {:>8}",
            "RUNS",
            "FIRST ACCEPT",
            "MEAN ROUNDS",
            "GATE FAIL",
            "CHANGES",
            "MEDIAN WALL(s)",
            "DISCARDS"
        );
        if tokens {
            print!(" {:>13} {:>12}", "MEDIAN TOKENS", "TOTAL TOKENS");
        }
        if costs {
            print!(" {:>12} {:>12}", "MEDIAN USD", "TOTAL USD");
        }
        if estimates {
            print!(
                " {:>14} {:>13} {:>9}",
                "MEDIAN EST USD", "TOTAL EST USD", "BASIS"
            );
        }
        if unpriced {
            print!(" {:>15}", "UNPRICED TOKENS");
        }
        println!();
        for r in rows {
            let rate = |v: Option<f64>| {
                v.map(|v| format!("{:.1}%", v * 100.0))
                    .unwrap_or_else(|| "-".into())
            };
            let number =
                |v: Option<f64>| v.map(|v| format!("{v:.1}")).unwrap_or_else(|| "-".into());
            print!(
                "{:<20} {:>5} {:>12} {:>11} {:>10} {:>8} {:>14} {:>8}",
                r.group,
                r.runs,
                rate(r.first_round_accept_rate),
                number(r.mean_rounds_to_accept),
                rate(r.gate_failure_rate),
                r.changes_requested,
                number(r.median_wall_seconds),
                r.discards
            );
            if tokens {
                print!(
                    " {:>13} {:>12}",
                    number(r.median_tokens_per_run),
                    r.total_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into())
                );
            }
            if costs {
                let usd =
                    |v: Option<f64>| v.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into());
                print!(
                    " {:>12} {:>12}",
                    usd(r.median_cost_usd_per_run),
                    usd(r.total_cost_usd)
                );
            }
            if estimates {
                let usd =
                    |v: Option<f64>| v.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into());
                let basis = match r.cost_basis {
                    Some(corral_core::CostBasis::Reported) => "reported",
                    Some(corral_core::CostBasis::Estimated) => "estimated",
                    Some(corral_core::CostBasis::Mixed) => "mixed",
                    None => "-",
                };
                print!(
                    " {:>14} {:>13} {:>9}",
                    usd(r.median_estimated_usd_per_run),
                    usd(r.total_estimated_usd),
                    basis
                );
            }
            if unpriced {
                print!(
                    " {:>15}",
                    r.unpriced_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into())
                );
            }
            println!();
            let lessons = &r.lesson_effectiveness;
            if lessons.exposed.runs + lessons.unexposed.runs > 0 {
                print!("  lessons (runs/accepted, mean rounds, gate fail):");
                for (name, cohort) in [
                    ("exposed", &lessons.exposed),
                    ("unexposed", &lessons.unexposed),
                    ("unknown", &lessons.unknown),
                ] {
                    print!(
                        " {name} {}/{} {} {}({}/{})",
                        cohort.runs,
                        cohort.accepted,
                        number(cohort.mean_rounds_to_accept),
                        rate(cohort.gate_failure_rate),
                        cohort.gate_failures,
                        cohort.gate_attempts
                    );
                }
                println!();
            }
        }
        if by_kind == StatsBy::Mode {
            print_mode_throughput(ctx, cutoff)?;
        }
    }
    Ok(())
}

fn write_html_report(
    ctx: &Ctx,
    by: StatsBy,
    cutoff: Option<jiff::Timestamp>,
    out: Option<&std::path::Path>,
) -> Result<()> {
    if by != StatsBy::Mode {
        bail!("--html reports throughput by mode; pass --by mode");
    }
    let page = throughput_report_html(&mode_throughput(&ctx.store, cutoff)?);
    match out {
        Some(path) => std::fs::write(path, page)
            .with_context(|| format!("writing throughput report {}", path.display())),
        None => {
            print!("{page}");
            Ok(())
        }
    }
}

fn print_mode_throughput(ctx: &Ctx, cutoff: Option<jiff::Timestamp>) -> Result<()> {
    let rows = mode_throughput(&ctx.store, cutoff)?;
    let labels: Vec<String> = rows.iter().map(|r| mode_label(&r.group)).collect();
    let width = labels
        .iter()
        .map(String::len)
        .fold("MODE (RESPONSE MODEL)".len(), usize::max);
    println!(
        "\n{:<width$} {:>7} {:>8} {:>9} {:>9} {:>10}",
        "MODE (RESPONSE MODEL)", "SAMPLES", "EXCLUDED", "P50 TOK/S", "P90 TOK/S", "EST USD/HR"
    );
    for (row, label) in rows.iter().zip(&labels) {
        let t = &row.throughput;
        println!(
            "{label:<width$} {:>7} {:>8} {:>9} {:>9} {:>10}",
            t.samples,
            t.excluded,
            fixed(t.p50_tokens_per_second, 1),
            fixed(t.p90_tokens_per_second, 1),
            fixed(t.estimated_usd_per_active_hour, 2)
        );
    }
    println!("\n{RATE_NOTE}");
    println!(
        "Rates cover implementation responses, each under its own response model; \
         EXCLUDED counts usage entries without timing."
    );
    Ok(())
}

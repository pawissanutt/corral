use crate::cmds::route::run_profile;
use anyhow::{Context, Result, bail};
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

pub fn run(ctx: &Ctx, by: &str, since: Option<&str>, json: bool) -> Result<()> {
    let by_kind = match by {
        "profile" => StatsBy::Profile,
        "model" => StatsBy::Model,
        "kind" => StatsBy::Kind,
        "project" => StatsBy::Project,
        "task" => StatsBy::Task,
        _ => bail!("unknown stats grouping {by}"),
    };
    let cutoff = since.map(since_cutoff).transpose()?;
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
        }
    }
    Ok(())
}

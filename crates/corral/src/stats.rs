use anyhow::{Context, Result, bail};
use corral_ops::{Ctx, StatsBy};

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
        _ => bail!("unknown stats grouping {by}"),
    };
    let cutoff = since.map(since_cutoff).transpose()?;
    let rows = corral_ops::stats(&ctx.store, by_kind, cutoff)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        let tokens = rows.iter().any(|r| r.total_tokens.is_some());
        let costs = rows.iter().any(|r| r.total_cost_usd.is_some());
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
            println!();
        }
    }
    Ok(())
}

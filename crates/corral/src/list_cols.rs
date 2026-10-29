//! The plain `corral list` rows, and `--columns`: the same fields the board cards can show.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use corral_core::{CardField, PinRef, Task};
use corral_ops::Ctx;
use jiff::Timestamp;

const BASE: [&str; 4] = ["id", "status", "project", "title"];

fn cell(name: &str, t: &Task, all: &[Task], pinned: &BTreeSet<String>, now: Timestamp) -> String {
    let text = match name {
        "id" => Some(t.id.clone()),
        "status" => Some(t.status.label().to_string()),
        "project" => t.project.clone(),
        "title" => Some(t.title.clone()),
        field => CardField::parse(field).and_then(|f| f.text(t, all, now, pinned.contains(&t.id))),
    };
    text.unwrap_or_else(|| "-".to_string())
}

pub fn print(ctx: &Ctx, tasks: &[Task], columns: &[String]) -> Result<()> {
    let now = Timestamp::now();
    let assessments: BTreeMap<_, _> = corral_ops::attention::board(ctx, now)?
        .tasks
        .into_iter()
        .map(|row| (row.task, row.assessment))
        .collect();
    let blocker = |task: &Task| {
        assessments
            .get(&task.id)
            .and_then(|assessment| corral_core::attention::blocked(task, assessment))
    };
    if columns.is_empty() {
        for t in tasks {
            let blocked = blocker(t);
            println!(
                "{:<7} {:<9} {:<3} {:<14} {}{}",
                t.id,
                blocked.map_or(t.status.label(), |_| "Blocked"),
                t.priority,
                t.project.as_deref().unwrap_or("-"),
                t.title,
                blocked
                    .map(|condition| format!("  [{}]", blocked_text(condition)))
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }
    if let Some(bad) = columns
        .iter()
        .find(|c| !BASE.contains(&c.as_str()) && CardField::parse(c).is_none())
    {
        let known: Vec<&str> = BASE
            .into_iter()
            .chain(CardField::ALL.iter().map(|f| f.key()))
            .collect();
        bail!("unknown column '{bad}' (available: {})", known.join(", "));
    }
    let all = ctx.store.list()?;
    let pinned: BTreeSet<String> = ctx
        .store
        .pins()?
        .iter()
        .filter_map(|p| PinRef::parse(&p.target).ok())
        .map(|r| r.task().to_string())
        .collect();
    let rows: Vec<Vec<String>> = tasks
        .iter()
        .map(|t| {
            columns
                .iter()
                .map(|c| match (c.as_str(), blocker(t)) {
                    ("status", Some(_)) => "Blocked".into(),
                    ("blocked" | "needs_me", Some(condition)) => blocked_text(condition),
                    _ => cell(c, t, &all, &pinned, now),
                })
                .collect()
        })
        .collect();
    let widths: Vec<usize> = (0..columns.len())
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    for row in &rows {
        let last = row.len() - 1;
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i == last {
                    v.clone()
                } else {
                    format!("{v:<w$}", w = widths[i])
                }
            })
            .collect();
        println!("{}", line.join("  "));
    }
    Ok(())
}

fn blocked_text(condition: &corral_core::attention::Condition) -> String {
    format!(
        "{}; unblocks: {}; {}",
        condition
            .age_secs
            .map_or_else(|| "age unknown".into(), corral_core::age_text),
        condition.unblocker(),
        condition
            .reason
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
}

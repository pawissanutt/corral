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
    let assessments: BTreeMap<_, _> = corral_ops::attention::board_hot(ctx, now)?
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

pub fn print_heads(ctx: &Ctx, heads: &[corral_core::TaskHead], columns: &[String]) -> Result<()> {
    if columns.is_empty() {
        let assessments: BTreeMap<_, _> = corral_ops::attention::board_hot(ctx, Timestamp::now())?
            .tasks
            .into_iter()
            .map(|row| (row.task, row.assessment))
            .collect();
        for head in heads {
            let blocked = assessments
                .get(&head.id)
                .map(|assessment| corral_core::attention::blocked_ref(head, assessment))
                .transpose()?
                .flatten();
            println!(
                "{:<7} {:<9} {:<3} {:<14} {}{}",
                head.id,
                blocked.map_or(head.status.label(), |_| "Blocked"),
                head.priority,
                head.project.as_deref().unwrap_or("-"),
                head.title,
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
        let known: Vec<_> = BASE
            .into_iter()
            .chain(CardField::ALL.iter().map(|f| f.key()))
            .collect();
        bail!("unknown column '{bad}' (available: {})", known.join(", "));
    }
    let ids = heads
        .iter()
        .flat_map(|h| h.depends_on.iter())
        .cloned()
        .collect();
    let done: BTreeSet<_> = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Ids(ids))?
        .into_iter()
        .filter(|h| h.status == corral_core::Status::Done)
        .map(|h| h.id.clone())
        .collect();
    let pinned: BTreeSet<_> = ctx
        .store
        .pins()?
        .iter()
        .filter_map(|p| PinRef::parse(&p.target).ok())
        .map(|r| r.task().to_owned())
        .collect();
    let now = Timestamp::now();
    let assessments: BTreeMap<_, _> = if columns
        .iter()
        .any(|c| matches!(c.as_str(), "status" | "blocked" | "needs_me"))
    {
        corral_ops::attention::board_hot(ctx, now)?
            .tasks
            .into_iter()
            .map(|row| (row.task, row.assessment))
            .collect()
    } else {
        BTreeMap::new()
    };
    let mut rows = Vec::new();
    for head in heads {
        let blocked = assessments
            .get(&head.id)
            .map(|assessment| corral_core::attention::blocked_ref(head, assessment))
            .transpose()?
            .flatten();
        rows.push(
            columns
                .iter()
                .map(|name| match (name.as_str(), blocked) {
                    ("status", Some(_)) => "Blocked".into(),
                    ("blocked" | "needs_me", Some(condition)) => blocked_text(condition),
                    _ => head_cell(name, head, &done, &pinned, now),
                })
                .collect::<Vec<_>>(),
        );
    }
    let widths: Vec<_> = (0..columns.len())
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    for row in rows {
        let last = row.len() - 1;
        println!(
            "{}",
            row.into_iter()
                .enumerate()
                .map(|(i, text)| {
                    if i == last {
                        text
                    } else {
                        format!("{text:<w$}", w = widths[i])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
        );
    }
    Ok(())
}

fn head_cell(
    name: &str,
    head: &corral_core::TaskHead,
    done: &BTreeSet<String>,
    pinned: &BTreeSet<String>,
    now: Timestamp,
) -> String {
    match name {
        "id" => head.id.clone(),
        "status" => head.status.label().to_owned(),
        "project" => head.project.clone().unwrap_or_else(|| "-".into()),
        "title" => head.title.clone(),
        field => CardField::parse(field)
            .and_then(|f| {
                f.text_head_with(
                    head,
                    &[],
                    &|id| done.contains(id),
                    now,
                    pinned.contains(&head.id),
                )
            })
            .unwrap_or_else(|| "-".into()),
    }
}

#[cfg(test)]
mod artifact_facts_list {
    use super::*;
    use clap::{CommandFactory, FromArgMatches};
    use corral_core::work::{self, Counter};
    use corral_core::{Artifact, Status, Store};

    #[test]
    fn open_list_artifact_columns_are_constant_under_current_and_reviewed_history_growth() {
        let mut reports = Vec::new();
        for factor in [1, 10] {
            // Given a current run's growing artifacts and reviewed terminal history.
            let root = tempfile::tempdir().unwrap();
            let store = Store::init(root.path()).unwrap();
            let mut task = store.create("Active", None, Status::Running, "").unwrap();
            let now: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
            task.created = now;
            task.updated = now;
            let run: corral_core::Run = serde_json::from_value(serde_json::json!({
                "id": "R1", "agent": "worker", "worktree": "/tmp/list-artifact-test",
                "branch": "topic", "base": "main", "base_sha": "0".repeat(40), "started": now
            }))
            .unwrap();
            task.runs.push(run);
            let artifact = |run: &str, path: String, kind: &str, title: &str| -> Artifact {
                serde_json::from_value(serde_json::json!({
                    "run": run, "path": path, "kind": kind, "title": title,
                    "source": "/tmp/x", "sha256": "0".repeat(64), "bytes": 1, "added": now
                }))
                .unwrap()
            };
            for n in 0..4 * factor {
                task.artifacts.push(artifact(
                    "R1",
                    format!("artifacts/{}/R1/{n:04}", task.id),
                    "log",
                    "Log",
                ));
            }
            task.artifacts.push(artifact(
                "R1",
                "artifacts/final.diff".into(),
                "diff",
                "Final diff +12 -3",
            ));
            store.fixture_put_task(&task).unwrap();
            let mut marks = BTreeSet::new();
            for n in 0..4 * factor {
                let mut old = store.create("Old", None, Status::Dropped, "").unwrap();
                let path = format!("artifacts/{}/R0/{n:04}", old.id);
                marks.insert(path.clone());
                old.artifacts.push(artifact("R0", path, "report", "Report"));
                store.fixture_put_task(&old).unwrap();
            }
            corral_core::records::RecordArea::new("reviewed-artifacts")
                .save(&store, "set", &marks)
                .unwrap();
            let ctx = Ctx {
                store,
                herdr: corral_herdr::Herdr::with_bin(root.path().join("unused-herdr"), None),
                resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            };
            let columns = ["id", "artifacts", "lines"].map(str::to_owned);
            let before = work::snapshot();
            // When the actual open-list command evaluates artifact columns. Its production
            // HotScope excludes CLI attribution's deferred FU-2 worktree discovery.
            {
                let matches = crate::Cli::command()
                    .try_get_matches_from([
                        "corral",
                        "--root",
                        root.path().to_str().unwrap(),
                        "list",
                        "--columns",
                        "id,artifacts,lines",
                    ])
                    .unwrap();
                let cli = crate::Cli::from_arg_matches(&matches).unwrap();
                crate::cmds::run(cli, &matches).unwrap();
            }
            // Then every command cell equals the full-task oracle, with identical work.
            let report = work::snapshot().since(&before);
            assert_eq!(report.unattributed_hydrations(), 0);
            report.check_identity().unwrap();
            reports.push(report);
            let head = ctx.store.head(&task.id).unwrap();
            let empty = BTreeSet::new();
            for column in &columns {
                assert_eq!(
                    head_cell(column, &head, &empty, &empty, now),
                    cell(column, &task, &[], &empty, now)
                );
            }
        }
        for counter in Counter::ALL {
            assert_eq!(
                reports[0].get(counter),
                reports[1].get(counter),
                "{counter:?}"
            );
        }
    }
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

//! Cold rebuilds and incremental maintenance. Only keyed changed tasks, bounded recent
//! events and each affected project's latest 256 sources are read on an ordinary update.

use std::collections::BTreeSet;
use std::fs;

use corral_core::backend::{ChangeScope, EventQuery, EventSeq, HeadQuery, StoredEvent};

use super::*;
use crate::approvals::{changes, event, events};

const SCHEDULE_KINDS: [&str; 10] = [
    "lessons_distill_requested",
    "lessons_due",
    "approved",
    "gate",
    "changes_requested",
    "discarded",
    "escalated",
    "autopilot",
    "plan_escalated",
    "plan_failed",
];

pub(super) fn prepare_locked(ctx: &Ctx, selected: &BTreeSet<String>) -> Result<Prepared> {
    prepare_at(ctx, selected, root(ctx)?, true)
}

pub(super) fn prepare_schedule_locked(ctx: &Ctx, selected: &BTreeSet<String>) -> Result<Prepared> {
    prepare_at(ctx, selected, schedule_root(ctx)?, false)
}

fn prepare_at(
    ctx: &Ctx,
    selected: &BTreeSet<String>,
    root: PathBuf,
    signals: bool,
) -> Result<Prepared> {
    // The cursor is invalidated before any cache writes. On expiry a retry rebuilds
    // from committed inputs, never reapplies incremental counter updates to partial rows.
    ctx.read_step(|read| prepare_view(read, selected, root.clone(), signals))
}

fn prepare_view(
    ctx: &Ctx,
    selected: &BTreeSet<String>,
    root: PathBuf,
    signals: bool,
) -> Result<Prepared> {
    let path = root.join("cursor.json");
    let ctx = Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    let seq = ctx.store.view()?.seq();
    let config = crate::verification::digest(&serde_json::to_vec(&ctx.store.config.projects)?);
    let previous: Option<Cursor> = read(&path)?;
    let delta = previous
        .as_ref()
        .filter(|c| c.version == 6 && c.change <= seq && c.config == config)
        .map(|c| changes(&ctx.store, c.change))
        .transpose()?
        .flatten();
    let rebuild = previous.is_none() || delta.is_none();
    let mut cursor = if rebuild {
        Cursor::default()
    } else {
        previous.unwrap_or_default()
    };
    if !rebuild && cursor.change == seq {
        return Prepared::capture(&root, selected);
    }
    // The marker is removed before any keyed writes. An interrupted update is recovered
    // by a cold rebuild rather than replaying non-idempotent counter increments.
    if path.exists() {
        fs::remove_file(&path).context("invalidating lesson row cursor")?;
    }
    if rebuild && root.exists() {
        fs::remove_dir_all(&root).context("rebuilding lesson rows")?;
    }
    let mut changed = BTreeSet::new();
    let mut new_events = rebuild;
    for scope in delta.into_iter().flatten() {
        match scope {
            ChangeScope::Task { num, .. } => {
                changed.insert(corral_core::Store::normalize_id(&num.0.to_string())?);
            }
            ChangeScope::Artifact { task } => {
                changed.insert(task);
            }
            ChangeScope::Events { .. } => new_events = true,
            _ => {}
        }
    }
    let mut projects = ctx
        .store
        .config
        .projects
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut affected = BTreeSet::new();
    if rebuild {
        for query in [
            HeadQuery::Open { project: None },
            HeadQuery::RecentByStatus {
                status: "done".into(),
            },
            HeadQuery::RecentByStatus {
                status: "dropped".into(),
            },
        ] {
            for head in ctx.store.heads(&query)? {
                let meta = Meta::of(&head);
                if let Some(project) = &meta.project {
                    projects.insert(project.clone());
                }
                schedule::task_changed(&root, &head.id, None, Some(&meta))?;
                save(&task_path(&root, &head.id), &meta)?;
            }
        }
        affected.extend(projects.iter().cloned());
        for event in events(
            &ctx.store,
            &EventQuery {
                kinds: SCHEDULE_KINDS.map(str::to_owned).into(),
                valid_only: true,
                ..EventQuery::default()
            },
        )? {
            schedule::apply(&root, &projects, &event)?;
        }
    } else {
        for id in &changed {
            let path = task_path(&root, id);
            let before: Option<Meta> = read(&path)?;
            let head = ctx.store.heads(&HeadQuery::Ids(vec![id.clone()]))?.pop();
            let after = head.as_ref().map(Meta::of);
            for meta in [before.as_ref(), after.as_ref()].into_iter().flatten() {
                if let Some(project) = &meta.project {
                    projects.insert(project.clone());
                    affected.insert(project.clone());
                }
            }
            schedule::task_changed(&root, id, before.as_ref(), after.as_ref())?;
            match after {
                Some(meta) => save(&path, &meta)?,
                None if before.is_some() => {
                    fs::remove_file(&path).context("removing deleted lesson task metadata")?
                }
                None => {}
            }
        }
    }
    // The baseline's newest-4-MiB event window is kept as a bound (`window_after`) and its
    // byte total, never as a copied event list: each event is read once on entry and once
    // on eviction, and a refresh reads only its window tasks' own events.
    let limit = u64::try_from(EVENT_BYTES)?;
    if rebuild && signals {
        (cursor.window_after, cursor.window_bytes) = window(&ctx, limit)?;
    } else if !rebuild && new_events {
        for stored in event_rows(
            &ctx.store,
            &EventQuery {
                after: Some(EventSeq(cursor.event)),
                ..EventQuery::default()
            },
        )? {
            cursor.window_bytes += u64::try_from(serde_json::to_vec(&stored.event)?.len())?;
            if let Some(event) = event(stored)? {
                schedule::apply(&root, &projects, &event)?;
                affect(&root, &event, &mut affected)?;
            }
        }
        let view = ctx.store.view()?;
        while signals && cursor.window_bytes > limit {
            let query = EventQuery {
                after: after(cursor.window_after),
                ..EventQuery::default()
            };
            let Some(oldest) = view.events(&query, PageReq::first(1))?.items.pop() else {
                break;
            };
            cursor.window_bytes -= u64::try_from(serde_json::to_vec(&oldest.event)?.len())?;
            cursor.window_after = oldest.seq.0;
            if let Some(event) = event(oldest)? {
                affect(&root, &event, &mut affected)?;
            }
        }
    }
    if new_events {
        cursor.event = ctx.store.event_tail()?;
    }
    if signals {
        for project in &affected {
            refresh_signals(&ctx, &root, project, &changed, rebuild, cursor.window_after)?;
            cursor.recomputations += 1;
        }
    }
    // Scheduling rows are constant-size and project-keyed. A new event only updates the
    // relevant counters above; this output step never visits historical task metadata.
    if new_events || !changed.is_empty() || rebuild {
        for project in &projects {
            let row: schedule::Project = read(&project_path(&root, project))?.unwrap_or_default();
            save(&root.join(format!("due-{}.json", key(project))), &row.due())?;
        }
    }
    cursor.change = seq;
    cursor.config = config;
    cursor.version = 6;
    save(&path, &cursor)?;
    Prepared::capture(&root, selected)
}

fn affect(root: &Path, event: &Event, affected: &mut BTreeSet<String>) -> Result<()> {
    if matches!(
        event.kind.as_str(),
        "approved"
            | "done"
            | "discarded"
            | "changes_requested"
            | "verdict"
            | "gate"
            | "needs_me"
            | "escalated"
            | "blocked"
            | "failed"
            | "round"
            | "autopilot"
    ) && let Some(id) = &event.task
        && let Some(meta) = read::<Meta>(&task_path(root, id))?
        && let Some(project) = meta.project
    {
        affected.insert(project);
    }
    Ok(())
}

fn refresh_signals(
    ctx: &Ctx,
    root: &Path,
    project: &str,
    changed: &BTreeSet<String>,
    rebuild: bool,
    window_after: u64,
) -> Result<()> {
    #[derive(Serialize, Deserialize)]
    struct Cached {
        rev: u64,
        source: Source,
    }
    let heads = ctx.store.heads_page(
        &HeadQuery::ProjectUpdated {
            project: project.to_owned(),
        },
        u32::try_from(TASK_LIMIT)?,
    )?;
    let mut sources = Vec::new();
    let mut landed = BTreeMap::new();
    let mut recent = Vec::new();
    for head in heads {
        let task_events = event_rows(
            &ctx.store,
            &EventQuery {
                task: Some(head.id.clone()),
                after: after(window_after),
                ..EventQuery::default()
            },
        )?
        .into_iter()
        .map(event)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let path = root.join("sources").join(format!("{}.json", key(&head.id)));
        let cached: Option<Cached> = if rebuild || changed.contains(&head.id) {
            None
        } else {
            read(&path)?
        };
        let source = match cached.filter(|cached| cached.rev == head.rev) {
            Some(cached) => cached.source,
            None => {
                let source = source(ctx, &head)?;
                save(
                    &path,
                    &Cached {
                        rev: head.rev,
                        source: source.clone(),
                    },
                )?;
                source
            }
        };
        let mut related = task_events
            .iter()
            .filter(|event| {
                event.task.as_deref() == Some(head.id.as_str())
                    && event.detail["project"]
                        .as_str()
                        .is_none_or(|p| p == project)
            })
            .collect::<Vec<_>>();
        related.sort_by(|a, b| {
            (a.ts, &a.kind, a.detail.to_string()).cmp(&(b.ts, &b.kind, b.detail.to_string()))
        });
        if let Some(event) = related
            .into_iter()
            .rev()
            .take(128)
            .find(|e| e.kind == "approved")
        {
            let path = root
                .join("landed")
                .join(key(project))
                .join(format!("{}.json", event.seq));
            let files = match read(&path)? {
                Some(files) => files,
                None => {
                    let files = landed_files(ctx, project, event)?;
                    save(&path, &files)?;
                    files
                }
            };
            landed.insert(event.seq, files);
        }
        sources.push(source);
        recent.extend(task_events);
    }
    let signals = crate::lesson_signals::collect(ctx, project, &sources, &recent, &landed)?;
    save(
        &root.join(format!("signals-{}.json", key(project))),
        &signals,
    )
}

fn event_rows(store: &corral_core::Store, query: &EventQuery) -> Result<Vec<StoredEvent>> {
    let view = store.view()?;
    let mut page = PageReq::first(256);
    let mut out = Vec::new();
    loop {
        let rows = view.events(query, page)?;
        out.extend(rows.items);
        match rows.next {
            Some(next) => {
                page = PageReq {
                    after: Some(next),
                    limit: 256,
                }
            }
            None => return Ok(out),
        }
    }
}

/// The baseline window: the newest events whose serialized size totals at most `limit`.
/// Returns the sequence just before the window (0 when it starts at the log's head) and
/// the window's byte total.
fn window(ctx: &Ctx, limit: u64) -> Result<(u64, u64)> {
    let view = ctx.store.view()?;
    let query = EventQuery {
        newest_first: true,
        ..EventQuery::default()
    };
    let mut page = PageReq::first(64);
    let mut bytes = 0;
    loop {
        let rows = view.events(&query, page)?;
        for row in rows.items {
            let size = u64::try_from(serde_json::to_vec(&row.event)?.len())?;
            if bytes + size > limit {
                return Ok((row.seq.0, bytes));
            }
            bytes += size;
        }
        match rows.next {
            Some(next) => {
                page = PageReq {
                    after: Some(next),
                    limit: 64,
                }
            }
            None => return Ok((0, bytes)),
        }
    }
}

fn after(seq: u64) -> Option<EventSeq> {
    (seq > 0).then_some(EventSeq(seq))
}

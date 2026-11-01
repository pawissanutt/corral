//! Terminal tasks that still owe container settlement, without a board scan: a Pi-native
//! container that is open or owes run-end duties, or an inbox container with unfinished
//! messages, must be visited once more after its task ends. The set follows the change feed
//! from a persisted cursor, so a pass reads only the tasks changed since the last one plus the
//! few still owing; a pruned feed or a new store falls back to one scan of the terminal tasks.

use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::backend::{ChangeScope, ChangeSeq, FeedError, PageReq, TaskOrder, TaskQuery};
use corral_core::inbox::{DeliveryMode, Inbox, Recipient};
use corral_core::{Status, Store, TaskHead};
use serde::{Deserialize, Serialize};

use crate::Ctx;

const AREA: &str = "terminal-settlement";

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct State {
    /// Zero-padded, so the record's size does not follow the feed's length.
    #[serde(with = "padded")]
    cursor: Option<u64>,
    tasks: BTreeSet<String>,
}

/// A change or event sequence stored zero-padded, so a record's size does not follow the
/// feed's length.
pub(crate) mod padded {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(cursor: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match cursor {
            Some(seq) => s.serialize_some(&format!("{seq:020}")),
            None => s.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|seq| seq.parse().map_err(serde::de::Error::custom))
            .transpose()
    }
}

/// The head as a task with its current containers, plus the current run's other open review
/// children before its current child: a superseded child still owes settlement, and closure
/// sees which child is newer.
pub(crate) fn shell_with_open_children(head: &TaskHead) -> Result<corral_core::Task> {
    let mut task = head.to_shell_task()?;
    if let (Some(current), Some(shell)) = (&head.current_run, task.runs.last_mut()) {
        let mut children = Vec::new();
        for child in head.open_review_children(&current.id)? {
            if current
                .current_child
                .as_ref()
                .is_none_or(|c| c.id != child.id)
            {
                children.push(serde_json::from_value(serde_json::to_value(&child)?)?);
            }
        }
        children.append(&mut shell.review_runs);
        shell.review_runs = children;
    }
    Ok(task)
}

/// The inbox recipients of a head's current containers that are not yet settled.
fn inbox_recipients(head: &TaskHead) -> Vec<Recipient> {
    let mut recipients = Vec::new();
    if let Some(run) = &head.current_run {
        if run.delivery == DeliveryMode::Inbox && run.inbox.settled.is_none() {
            recipients.push(Recipient::Run {
                task: head.id.clone(),
                run: run.id.clone(),
            });
        }
        if let Some(child) = run
            .current_child
            .as_deref()
            .filter(|c| c.delivery == DeliveryMode::Inbox && c.inbox.settled.is_none())
        {
            recipients.push(Recipient::ReviewRun {
                task: head.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            });
        }
    }
    if let Some(run) = head
        .last_plan_run
        .as_ref()
        .filter(|r| r.delivery == DeliveryMode::Inbox)
    {
        recipients.push(Recipient::PlanRun {
            task: head.id.clone(),
            plan_run: run.id.clone(),
        });
    }
    recipients
}

fn owes(ctx: &Ctx, head: &TaskHead) -> Result<bool> {
    if !matches!(head.status, Status::Done | Status::Dropped) {
        return Ok(false);
    }
    if crate::agent::hooks::sync::wants(&shell_with_open_children(head)?) {
        return Ok(true);
    }
    for recipient in inbox_recipients(head) {
        if !Inbox::new(&ctx.store, recipient)?
            .unfinished_messages()?
            .is_empty()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Every terminal task, by id: the recovery path when the feed cannot be followed.
fn terminal_ids(store: &Store) -> Result<BTreeSet<String>> {
    let view = store.view()?;
    let query = TaskQuery {
        statuses: vec!["done".into(), "dropped".into()],
        project: None,
        order: TaskOrder::IdAsc,
    };
    let mut ids = BTreeSet::new();
    let mut page = PageReq::first(500);
    loop {
        let found = view.tasks(&query, page.clone())?;
        ids.extend(found.items.into_iter().map(|header| header.id));
        match found.next {
            Some(next) => page.after = Some(next),
            None => return Ok(ids),
        }
    }
}

/// Recovery before a measured pass: a new store or a pruned feed scans the terminal tasks once
/// here, so the pass itself only follows the feed (plan T-253 section 4).
pub(crate) fn recover(ctx: &Ctx) -> Result<()> {
    let saved: State = ctx.store.record(AREA, "state")?.unwrap_or_default();
    let followable = match saved.cursor {
        Some(cursor) => !matches!(
            ctx.store.view()?.changes(ChangeSeq(cursor), 1),
            Err(FeedError::Gap { .. })
        ),
        None => false,
    };
    if !followable {
        terminal_heads(ctx)?;
    }
    Ok(())
}

/// Terminal heads that still owe settlement: the persisted set plus every task changed after
/// the cursor. The set and cursor are saved for the next pass when they change.
pub(crate) fn terminal_heads(ctx: &Ctx) -> Result<Vec<TaskHead>> {
    // Hooks in separate processes share this cursor. Hold the derivation lease from
    // the first read through the version-checked save, not just around publication.
    // It is released before callers reconcile recipients or make external calls.
    let _lease = crate::approvals::lock(&ctx.store, AREA)?;
    let saved: State = ctx.store.record(AREA, "state")?.unwrap_or_default();
    let mut candidates = saved.tasks.clone();
    let view = ctx.store.view()?;
    let seq = view.seq();
    // Whether the feed moved for anything but this set's own saves.
    let mut foreign = saved.cursor.is_none();
    match saved.cursor {
        Some(cursor) => {
            let mut after = ChangeSeq(cursor);
            loop {
                match view.changes(after, 256) {
                    Ok(page) => {
                        for entry in &page.entries {
                            after = entry.seq;
                            for scope in &entry.scopes {
                                match scope {
                                    ChangeScope::Task { num, .. } => {
                                        candidates.insert(Store::normalize_id(&num.0.to_string())?);
                                        foreign = true;
                                    }
                                    ChangeScope::Record { area, .. } if area == AREA => {}
                                    _ => foreign = true,
                                }
                            }
                        }
                        if !page.more {
                            break;
                        }
                    }
                    Err(FeedError::Gap { .. }) => {
                        candidates.extend(terminal_ids(&ctx.store)?);
                        foreign = true;
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        None => candidates.extend(terminal_ids(&ctx.store)?),
    }
    drop(view);
    let mut heads = Vec::new();
    let mut tasks = BTreeSet::new();
    for id in candidates {
        let Ok(head) = ctx.store.head(&id) else {
            continue;
        };
        if owes(ctx, &head)? {
            tasks.insert(id);
            heads.push(head);
        }
    }
    // Only this set's own saves since the cursor: re-saving would only move the feed again.
    if foreign || tasks != saved.tasks {
        let state = State {
            cursor: Some(seq.0),
            tasks,
        };
        if state != saved {
            ctx.store.save_record(AREA, "state", &state)?;
        }
    }
    Ok(heads)
}

//! The fake's read view: a pinned `Arc<State>` with an age check before every read.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use jiff::Timestamp;

use super::State;
use crate::backend::support::{Clock, elapsed};
use crate::backend::*;

pub(super) struct FakeView {
    pub(super) state: Arc<State>,
    pub(super) opened: Timestamp,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) max_age: Duration,
}

impl FakeView {
    fn fresh(&self) -> Result<(), ViewError> {
        let age = elapsed(self.opened, self.clock.now());
        if age > self.max_age {
            return Err(ViewError::Expired { age });
        }
        Ok(())
    }

    fn page<T>(&self, items: impl Iterator<Item = T>, req: &PageReq) -> Result<Page<T>, ViewError> {
        self.fresh()?;
        if req.limit == 0 {
            return Err(ViewError::Io(anyhow!("page limit must be positive")));
        }
        let offset = match &req.after {
            None => 0,
            Some(cursor) => cursor
                .parse::<usize>()
                .map_err(|_| ViewError::Io(anyhow!("invalid page cursor {cursor:?}")))?,
        };
        let limit = req.limit as usize;
        let mut items: Vec<T> = items.skip(offset).take(limit + 1).collect();
        let next = (items.len() > limit).then(|| {
            items.truncate(limit);
            (offset + limit).to_string()
        });
        Ok(Page { items, next })
    }

    fn status_counts(&self) -> Vec<StatusCount> {
        self.state
            .status_counts
            .iter()
            .map(|((status, project), n)| StatusCount {
                status: status.clone(),
                project: project.clone(),
                n: *n,
            })
            .collect()
    }

    fn check_feed(&self, after: ChangeSeq) -> Result<(), FeedError> {
        self.fresh()?;
        if after < self.state.feed_floor {
            return Err(FeedError::Gap {
                oldest: ChangeSeq(self.state.feed_floor.0 + 1),
            });
        }
        Ok(())
    }
}

impl ReadView for FakeView {
    fn seq(&self) -> ChangeSeq {
        self.state.seq
    }

    fn ensure_fresh(&self) -> Result<(), ViewError> {
        self.fresh()
    }

    fn board(&self, since: Option<ChangeSeq>) -> Result<BoardDelta, FeedError> {
        self.fresh()?;
        let open = |num: &TaskNum| {
            self.state
                .tasks
                .get(num)
                .filter(|task| task.head.header.is_open())
                .map(|task| task.head.clone())
        };
        let (heads, left) = match since {
            None => (
                self.state.tasks.keys().filter_map(open).collect(),
                Vec::new(),
            ),
            Some(since) => {
                self.check_feed(since)?;
                let changed: BTreeSet<TaskNum> = self
                    .state
                    .feed
                    .iter()
                    .filter(|entry| entry.seq > since)
                    .flat_map(|entry| entry.scopes.iter())
                    .filter_map(|scope| match scope {
                        ChangeScope::Task { num, .. } => Some(*num),
                        _ => None,
                    })
                    .collect();
                let heads = changed.iter().filter_map(open).collect();
                let left = changed
                    .into_iter()
                    .filter(|num| open(num).is_none())
                    .collect();
                (heads, left)
            }
        };
        Ok(BoardDelta {
            seq: self.state.seq,
            since,
            heads,
            left,
            status_counts: self.status_counts(),
        })
    }

    fn changes(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage, FeedError> {
        self.check_feed(after)?;
        if limit == 0 {
            return Err(FeedError::View(ViewError::Io(anyhow!(
                "page limit must be positive"
            ))));
        }
        let mut entries = self
            .state
            .feed
            .iter()
            .filter(|entry| entry.seq > after)
            .take(limit as usize + 1)
            .map(|entry| ChangeEntry::clone(entry));
        let page: Vec<ChangeEntry> = entries.by_ref().take(limit as usize).collect();
        let more = entries.next().is_some();
        Ok(ChangePage {
            entries: page,
            more,
        })
    }

    fn head(&self, num: TaskNum) -> Result<Option<TaskHeadDoc>, ViewError> {
        self.fresh()?;
        Ok(self.state.tasks.get(&num).map(|task| task.head.clone()))
    }

    fn heads(&self, q: &HeadQuery, page: PageReq) -> Result<Page<TaskHeadDoc>, ViewError> {
        let heads = self.state.tasks.values().map(|task| &task.head);
        match q {
            HeadQuery::Open { project } => self.page(
                heads
                    .filter(|head| head.header.is_open())
                    .filter(|head| project.is_none() || head.header.fields.project == *project)
                    .cloned(),
                &page,
            ),
            HeadQuery::Ids(ids) => {
                let ids: BTreeSet<&String> = ids.iter().collect();
                self.page(
                    heads.filter(|head| ids.contains(&head.header.id)).cloned(),
                    &page,
                )
            }
            HeadQuery::Children { parent } => self.page(
                heads
                    .filter(|head| head.header.fields.parent.as_ref() == Some(parent))
                    .cloned(),
                &page,
            ),
        }
    }

    fn task(&self, num: TaskNum, parts: &TaskParts) -> Result<Option<TaskDoc>, ViewError> {
        self.fresh()?;
        Ok(self.state.tasks.get(&num).map(|task| TaskDoc {
            head: task.head.clone(),
            rows: task
                .rows
                .values()
                .filter(|row| parts.includes(row.part))
                .cloned()
                .collect(),
        }))
    }

    fn rows(
        &self,
        num: TaskNum,
        q: &RowQuery,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        self.fresh()?;
        let Some(task) = self.state.tasks.get(&num) else {
            return self.page(std::iter::empty(), &page);
        };
        let rows = task
            .rows
            .range((q.part, String::new())..)
            .take_while(|((part, _), _)| *part == q.part)
            .map(|(_, row)| row)
            .filter(|row| q.matches(row));
        if q.newest_first {
            let mut rows: Vec<&HistoryRow> = rows.collect();
            rows.reverse();
            self.page(rows.into_iter().cloned(), &page)
        } else {
            self.page(rows.cloned(), &page)
        }
    }

    fn tasks(&self, q: &TaskQuery, page: PageReq) -> Result<Page<TaskHeader>, ViewError> {
        let mut headers: Vec<TaskHeader> = self
            .state
            .tasks
            .values()
            .map(|task| &task.head.header)
            .filter(|h| q.statuses.is_empty() || q.statuses.contains(&h.fields.status))
            .filter(|h| q.project.is_none() || h.fields.project == q.project)
            .cloned()
            .collect();
        if q.order == TaskOrder::UpdatedDesc {
            headers.sort_by(|a, b| {
                b.fields
                    .updated
                    .cmp(&a.fields.updated)
                    .then(a.num.cmp(&b.num))
            });
        }
        self.page(headers.into_iter(), &page)
    }

    fn events(&self, q: &EventQuery, page: PageReq) -> Result<Page<StoredEvent>, ViewError> {
        let ordered: Box<dyn Iterator<Item = &std::sync::Arc<StoredEvent>> + '_> = if q.newest_first
        {
            Box::new(self.state.events.iter().rev())
        } else {
            Box::new(self.state.events.iter())
        };
        let result = self.page(
            ordered
                .filter(|e| !q.valid_only || !e.malformed)
                .filter(|e| q.after.is_none_or(|after| e.seq > after))
                .filter(|e| q.task.is_none() || e.event.task == q.task)
                .filter(|e| q.kinds.is_empty() || q.kinds.contains(&e.event.kind))
                .filter(|e| q.run.is_none() || e.event.run == q.run)
                .map(|e| StoredEvent::clone(e)),
            &page,
        )?;
        for event in &result.items {
            let bytes =
                serde_json::to_vec(&event.event.detail).map_err(|e| ViewError::Io(e.into()))?;
            crate::work::decoded(
                crate::work::Counter::EventDecodes,
                u64::try_from(bytes.len()).map_err(|e| ViewError::Io(e.into()))?,
                crate::work::Subject::Plain,
            );
        }
        Ok(result)
    }

    fn conditions(
        &self,
        q: &ConditionQuery,
        page: PageReq,
    ) -> Result<Page<ConditionRun>, ViewError> {
        self.page(
            self.state
                .condition_runs
                .values()
                .filter(|run| q.task.as_ref().is_none_or(|task| run.task == *task))
                .filter(|run| q.kind.as_ref().is_none_or(|kind| run.kind == *kind))
                .filter(|run| !q.open_only || run.closed_seq.is_none())
                .cloned(),
            &page,
        )
    }

    fn records(&self, q: &RecordQuery, page: PageReq) -> Result<Page<StoredRecord>, ViewError> {
        self.page(
            self.state
                .records
                .values()
                .filter(|r| q.area.as_ref().is_none_or(|area| r.area == *area))
                .filter(|r| q.key.as_ref().is_none_or(|key| r.key == *key))
                .cloned(),
            &page,
        )
    }

    fn aggregates(&self, names: &[AggregateName]) -> Result<Aggregates, ViewError> {
        self.fresh()?;
        let mut out = Aggregates::default();
        if names.contains(&AggregateName::StatusCounts) {
            out.status_counts = self.status_counts();
        }
        if names.contains(&AggregateName::UsageTotals) {
            out.usage_totals = self
                .state
                .usage_totals
                .iter()
                .map(|((scope, key), doc)| UsageTotal {
                    scope: scope.clone(),
                    key: key.clone(),
                    doc: doc.clone(),
                })
                .collect();
        }
        if names.contains(&AggregateName::PaneOwners) {
            out.pane_owners = self
                .state
                .pane_owners
                .iter()
                .map(|(socket, n)| PaneOwner {
                    socket: socket.clone(),
                    n: *n,
                })
                .collect();
        }
        Ok(out)
    }

    fn blobs(&self, q: &BlobQuery, page: PageReq) -> Result<Page<BlobRow>, ViewError> {
        self.page(
            self.state
                .blobs
                .values()
                .filter(|b| q.hash.as_ref().is_none_or(|hash| b.hash == *hash))
                .filter(|b| q.state.is_none_or(|state| b.state == state))
                .cloned(),
            &page,
        )
    }

    fn storage_rows(&self, q: &StorageQuery, page: PageReq) -> Result<Page<StorageRow>, ViewError> {
        let state = &self.state;
        match q {
            StorageQuery::Scratch { states } => self.page(
                state
                    .scratch
                    .values()
                    .filter(|row| states.is_empty() || states.contains(&row.state))
                    .cloned()
                    .map(StorageRow::Scratch),
                &page,
            ),
            StorageQuery::Lifecycle { task } => self.page(
                state
                    .lifecycle
                    .values()
                    .filter(|row| task.as_ref().is_none_or(|task| row.task == *task))
                    .cloned()
                    .map(StorageRow::Lifecycle),
                &page,
            ),
            StorageQuery::Leases => {
                self.page(state.leases.values().cloned().map(StorageRow::Lease), &page)
            }
            StorageQuery::Claims => {
                self.page(state.claims.values().cloned().map(StorageRow::Claim), &page)
            }
            StorageQuery::Authorizations { task } => self.page(
                state
                    .authorizations
                    .values()
                    .filter(|row| task.as_ref().is_none_or(|task| row.task == *task))
                    .cloned()
                    .map(StorageRow::Authorization),
                &page,
            ),
        }
    }
}

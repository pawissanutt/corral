//! Scheduled work on the sync pass without a board listing (plan T-253 section 4, P7 sweep):
//! recurring occurrences come from the `tasks_dashboard_due` page, plan advances from open
//! heads, and the scheduled-digest check from the last digest event by kind. Each decides as
//! `db316644`'s listing or full event scan, on random boards and times, and does the same work
//! with 10x terminal tasks and 10x events.
use std::collections::BTreeSet;

use corral_core::backend::HeadQuery;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{DashboardSchedule, Event, Status, Store};
use serde_json::json;

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

/// Deterministic pseudo-random numbers from a seed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

const STATUSES: [Status; 5] = [
    Status::Done,
    Status::Backlog,
    Status::Ready,
    Status::Running,
    Status::Dropped,
];

/// Random scheduled tasks (due, not due, paused, absent; statuses with and without runs), then
/// `terminal` unscheduled Done tasks and `events` digest and other events.
fn board(root: &std::path::Path, seed: u64, terminal: usize, events: usize) -> Ctx {
    let mut rng = Lcg(seed);
    let ctx = Ctx::from_store(Store::init(root).unwrap());
    ctx.store.fixture_pin_now(at(0)).unwrap();
    for n in 0..12 {
        let status = STATUSES[rng.next(5) as usize];
        let mut task = ctx
            .store
            .create(&format!("Report {n}"), Some("app"), status, "")
            .unwrap();
        task.created = at(0);
        task.dashboard = match rng.next(4) {
            0 => None,
            k => Some(DashboardSchedule {
                interval_secs: 60,
                next_due: at(i64::try_from(rng.next(600)).unwrap()),
                paused: k == 3,
            }),
        };
        task.queued = rng.next(4) == 0;
        if rng.next(3) == 0 {
            let run = json!({"id": "R1", "agent": "claude", "worktree": root, "branch": "main",
                             "base": "main", "base_sha": "fixture", "started": at(0),
                             "ended": at(1), "outcome": "done"});
            task.runs.push(serde_json::from_value(run).unwrap());
        }
        ctx.store.fixture_put_task(&task).unwrap();
    }
    for n in 0..terminal {
        let mut task = ctx
            .store
            .create(&format!("Old {n}"), Some("app"), Status::Done, "")
            .unwrap();
        task.created = at(0);
        ctx.store.fixture_put_task(&task).unwrap();
    }
    let mut log = String::new();
    for e in 0..events {
        let event = Event {
            seq: 0,
            ts: at(i64::try_from(e).unwrap()),
            task: None,
            kind: if e % 3 == 0 { "digest" } else { "commented" }.into(),
            actor: None,
            via: None,
            detail: json!({"scheduled": e % 2 == 0, "snapshot_at": at(i64::try_from(e).unwrap())}),
        };
        log.push_str(&serde_json::to_string(&event).unwrap());
        log.push('\n');
    }
    ctx.store.fixture_append_events(&log).unwrap();
    ctx
}

/// `db316644`'s occurrence selection: the whole board listing, filtered by eligibility.
fn listed(ctx: &Ctx, now: Timestamp) -> BTreeSet<String> {
    ctx.store
        .list()
        .unwrap()
        .into_iter()
        .filter(|task| eligible(task, now))
        .map(|task| task.id)
        .collect()
}

fn paged(ctx: &Ctx, now: Timestamp) -> BTreeSet<String> {
    due_candidates(ctx, now)
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect()
}

#[test]
fn due_candidates_are_the_listing_s_eligible_tasks_at_every_time() {
    for seed in [3, 17, 99, 2024] {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = board(tmp.path(), seed, 3, 9);
        for now in [-10, 0, 150, 300, 450, 599, 700] {
            assert_eq!(
                paged(&ctx, at(now)),
                listed(&ctx, at(now)),
                "seed {seed} at {now}"
            );
        }
    }
}

/// The stored `dashboard_due` read back through the page: present exactly when the schedule
/// is present and unpaused, at exactly its `next_due`.
fn stored_due(ctx: &Ctx, id: &str) -> Option<Timestamp> {
    let due = |now: Timestamp| {
        ctx.store
            .heads(&HeadQuery::DashboardDue { now })
            .unwrap()
            .iter()
            .any(|head| head.id == id)
    };
    if !due(at(1_000_000)) {
        return None;
    }
    let (mut low, mut high) = (-1_000_000, 1_000_000);
    while low + 1 < high {
        let mid = (low + high) / 2;
        if due(at(mid)) {
            high = mid;
        } else {
            low = mid;
        }
    }
    Some(at(high))
}

fn expected_due(ctx: &Ctx, id: &str) -> Option<Timestamp> {
    ctx.store
        .head(id)
        .unwrap()
        .dashboard
        .as_ref()
        .filter(|schedule| !schedule.paused)
        .map(|schedule| schedule.next_due)
}

#[test]
fn dashboard_due_follows_every_schedule_write_pause_resume_and_occurrence() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(tmp.path()).unwrap());
    let ids: Vec<String> = (0..3)
        .map(|n| {
            ctx.store
                .create(&format!("Report {n}"), Some("app"), Status::Done, "")
                .unwrap()
                .id
        })
        .collect();
    let mut rng = Lcg(7);
    for step in 0..40 {
        let id = &ids[rng.next(3) as usize];
        let due = at(i64::try_from(rng.next(500)).unwrap());
        match rng.next(4) {
            0 => {
                configure(&ctx, id, 60, due, true).unwrap();
            }
            1 => {
                let _ = disable(&ctx, id, false);
            }
            2 => {
                let _ = disable(&ctx, id, true);
            }
            _ => {
                // An occurrence moves next_due on, as queue_due does.
                ctx.store
                    .update(id, |task| {
                        if let Some(schedule) = task.dashboard.as_mut() {
                            schedule.next_due = due;
                        }
                        Ok(())
                    })
                    .unwrap();
            }
        }
        for id in &ids {
            assert_eq!(
                stored_due(&ctx, id),
                expected_due(&ctx, id),
                "step {step} {id}"
            );
        }
    }
}

/// `db316644`'s scheduled-digest lookup: the whole digest event list, newest scheduled one.
fn scanned(store: &Store) -> Option<Event> {
    store
        .events_of(None, &["digest"])
        .unwrap()
        .into_iter()
        .rev()
        .find(|e| e.kind == "digest" && e.detail["scheduled"] == true)
}

#[test]
fn the_last_scheduled_digest_is_found_as_the_full_scan_finds_it() {
    for events in [0, 1, 2, 7, 30] {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = board(tmp.path(), 5, 0, events);
        let indexed = ctx
            .store
            .last_event_where(None, &["digest"], None, |e| e.detail["scheduled"] == true)
            .unwrap();
        assert_eq!(indexed, scanned(&ctx.store), "{events} events");
    }
}

fn last_scheduled(ctx: &Ctx) {
    ctx.store
        .last_event_where(None, &["digest"], None, |e| e.detail["scheduled"] == true)
        .unwrap();
}

fn unattributed(f: impl FnOnce()) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    f();
    let delta = work::snapshot().since(&before);
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn scheduled_work_does_not_grow_with_terminal_tasks_or_events() {
    let tmp = tempfile::tempdir().unwrap();
    // Equal-length roots: run worktrees are stored paths.
    let base = board(&tmp.path().join("small"), 11, 4, 12);
    let grown = board(&tmp.path().join("large"), 11, 40, 120);
    let now = at(300);
    assert_eq!(
        unattributed(|| {
            due_candidates(&base, now).unwrap();
        }),
        unattributed(|| {
            due_candidates(&grown, now).unwrap();
        })
    );
    assert_eq!(
        unattributed(
            || crate::plan_pilot::advance_all(&base, now, &crate::sync::SyncScope::All).unwrap()
        ),
        unattributed(
            || crate::plan_pilot::advance_all(&grown, now, &crate::sync::SyncScope::All).unwrap()
        )
    );
    assert_eq!(
        unattributed(|| last_scheduled(&base)),
        unattributed(|| last_scheduled(&grown))
    );
}

//! Owned daemon snapshots, independent of the transport and the UI runtime.
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// One process-lifetime copy of a task id, bounded by distinct ids received.
pub fn id(text: &str) -> &'static str {
    static IDS: LazyLock<Mutex<HashSet<&'static str>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    let mut ids = match IDS.lock() {
        Ok(ids) => ids,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(&existing) = ids.get(text) {
        return existing;
    }
    let text = Box::leak(text.to_owned().into_boxed_str());
    ids.insert(text);
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    /// No session has delivered a complete board yet and none has failed.
    #[default]
    Connecting,
    /// A session is open and a complete board has been applied.
    Up,
    /// Health and board remain the last ones received.
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    NeedsYou,
    Verifying,
    Producing,
    Queued,
    Held,
    Parked,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub mark: crate::model::board::Mark,
    pub sha7: Option<String>,
    /// Unix seconds.
    pub at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub session: Option<String>,
    pub role: String,
    pub profile: Option<String>,
    pub runtime: Option<String>,
    pub machine: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: &'static str,
    pub title: String,
    pub project: Option<String>,
    pub plan: Option<String>,
    pub goal: Option<String>,
    pub group: Group,
    /// Run, gate, review, land.
    pub rail: [Step; 4],
    pub now: String,
    pub now_code: String,
    /// Unix seconds since when `now` holds.
    pub since: Option<u64>,
    pub held_reason: Option<String>,
    pub questions: u64,
    pub attention: Vec<String>,
    pub agent: Option<Agent>,
    pub cost: Option<f64>,
    /// True when the cost is not wholly provider-reported.
    pub cost_estimated: bool,
    pub rev: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub epoch: u64,
    /// False until one complete snapshot has been applied since app startup.
    pub complete: bool,
    rows: Vec<Task>,
    pending_epoch: Option<u64>,
}

impl Board {
    pub fn tasks(&self) -> &[Task] {
        &self.rows
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.rows
            .binary_search_by(|task| order(task.id, id))
            .ok()
            .map(|index| &self.rows[index])
    }

    pub fn in_group(&self, group: Group) -> impl Iterator<Item = &Task> {
        self.rows.iter().filter(move |task| task.group == group)
    }

    pub fn count(&self, group: Group) -> usize {
        self.in_group(group).count()
    }

    /// Keep the previous snapshot visible until the replacement is complete.
    pub fn begin_epoch(&mut self, epoch: u64) {
        self.pending_epoch = Some(epoch);
    }

    /// Apply only complete batches: page accumulation belongs to the session.
    pub fn apply_batch(&mut self, epoch: u64, rows: Vec<Task>, left: &[String]) {
        if !self.complete || self.epoch != epoch || self.pending_epoch == Some(epoch) {
            self.rows.clear();
        }
        for row in rows {
            match self
                .rows
                .binary_search_by(|existing| order(existing.id, row.id))
            {
                Ok(index) => self.rows[index] = row,
                Err(index) => self.rows.insert(index, row),
            }
        }
        if !left.is_empty() {
            let left: HashSet<&str> = left.iter().map(String::as_str).collect();
            self.rows.retain(|row| !left.contains(row.id));
        }
        self.epoch = epoch;
        self.complete = true;
        self.pending_epoch = None;
    }
}

fn order(a: &str, b: &str) -> Ordering {
    fn number(id: &str) -> Option<u64> {
        id.strip_prefix("T-")?.parse().ok()
    }
    match (number(a), number(b)) {
        (Some(a_num), Some(b_num)) => a_num.cmp(&b_num).then_with(|| a.cmp(b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonLevel {
    Healthy,
    Degraded,
    Down,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLevel {
    Ok,
    Unreachable,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmLevel {
    Live,
    Relaunching,
    Absent,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub kind: String,
    pub level: RuntimeLevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonHealth {
    pub level: DaemonLevel,
    pub reasons: Vec<String>,
    pub runtimes: Vec<Runtime>,
    pub pm: PmLevel,
    pub pass_seq: u64,
    /// Unix seconds of the last completed pass.
    pub last_pass: Option<u64>,
    pub last_pass_ms: Option<u64>,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Feed {
    pub link: Link,
    /// A one-line connection problem; absent while up.
    pub problem: Option<String>,
    pub health: Option<DaemonHealth>,
    pub board: Board,
    pub now: u64,
    /// Local wall clock of `now`, HH:MM.
    pub clock: String,
    /// Local wall clock of the last daemon message.
    pub synced: Option<String>,
    /// Store directory path reported by the daemon, e.g. `/workspace/.corral`.
    pub store: Option<String>,
    /// Parent directory of `store`, when it has one.
    pub workspace: Option<String>,
    /// This device's hostname, captured once when the client starts.
    pub host: Option<String>,
    pub socket: Option<String>,
}

impl Feed {
    pub fn connecting(&mut self, socket: String) {
        self.socket = Some(socket);
        self.link = Link::Connecting;
        self.problem = Some("Waiting for the daemon board".into());
    }

    pub fn welcome_received(&mut self, store: String, health: DaemonHealth) {
        self.workspace = std::path::Path::new(&store)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .and_then(|parent| parent.to_str())
            .map(str::to_owned);
        self.store = Some(store);
        self.health_received(health);
    }

    pub fn health_received(&mut self, health: DaemonHealth) {
        self.health = Some(health);
        self.synced = Some(self.clock.clone());
    }

    pub fn begin_epoch(&mut self, epoch: u64) {
        self.board.begin_epoch(epoch);
    }

    pub fn board_received(
        &mut self,
        epoch: u64,
        rows: Vec<Task>,
        left: &[String],
        replacement: bool,
    ) {
        if replacement {
            self.begin_epoch(epoch);
        }
        self.board.apply_batch(epoch, rows, left);
        self.link = Link::Up;
        self.problem = None;
        self.synced = Some(self.clock.clone());
    }

    pub fn link_lost(&mut self, problem: String) {
        self.link = Link::Down;
        self.problem = Some(if problem.contains(['\r', '\n']) {
            problem.replace(['\r', '\n'], " ")
        } else {
            problem
        });
    }

    pub fn tick(&mut self, now: u64, clock: String) {
        self.now = now;
        self.clock = clock;
    }
}

/// Whole-unit age: `12s`, `4m`, `3h`, `2d`; empty when unknown.
pub fn age(since: Option<u64>, now: u64) -> String {
    let Some(since) = since else {
        return String::new();
    };
    let seconds = now.saturating_sub(since);
    let (amount, unit) = match seconds {
        0..60 => (seconds, "s"),
        60..3600 => (seconds / 60, "m"),
        3600..86400 => (seconds / 3600, "h"),
        _ => (seconds / 86400, "d"),
    };
    format!("{amount}{unit}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::board::Mark;

    fn task(text: &str) -> Task {
        Task {
            id: id(text),
            title: text.into(),
            project: None,
            plan: None,
            goal: None,
            group: Group::Queued,
            rail: std::array::from_fn(|_| Step {
                mark: Mark::Next,
                sha7: None,
                at: None,
            }),
            now: "queued".into(),
            now_code: "queued".into(),
            since: None,
            held_reason: None,
            questions: 0,
            attention: vec![],
            agent: None,
            cost: None,
            cost_estimated: false,
            rev: 1,
        }
    }

    #[test]
    fn board_batch_removes_left_and_keeps_numeric_order() {
        let mut board = Board::default();
        board.apply_batch(1, vec![task("T-10"), task("T-9"), task("other")], &[]);
        board.apply_batch(1, vec![task("T-2")], &["T-10".into()]);
        assert_eq!(
            board.tasks().iter().map(|row| row.id).collect::<Vec<_>>(),
            ["T-2", "T-9", "other"]
        );
        assert!(board.task("T-10").is_none());
    }

    #[test]
    fn epoch_replacement_keeps_old_rows_until_snapshot_completes() {
        let mut feed = Feed::default();
        feed.board_received(1, vec![task("T-9")], &[], true);
        feed.begin_epoch(2);
        assert_eq!(feed.board.epoch, 1);
        assert!(feed.board.task("T-9").is_some());
        feed.board_received(2, vec![task("T-10")], &[], false);
        assert_eq!(feed.board.epoch, 2);
        assert!(feed.board.task("T-9").is_none());
        assert!(feed.board.task("T-10").is_some());
        assert_eq!(feed.link, Link::Up);
    }
}

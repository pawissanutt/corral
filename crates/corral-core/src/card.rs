//! Board card fields: the `[board.card]` config and the field catalog shared by the board and `corral list
//! --columns`, so both surfaces format a task the same way.
//!
//! Cost labels: `$1.23` is spend the harness reported; `sub ~$1.23` is the API-equivalent estimate from the
//! pricing table (subscription runs report no spend). A task with runs but no priced usage shows `$—`; one that
//! never ran shows `$0.00`.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{ArtifactKind, AutoStage, Config, Status, Task};

/// One selectable card field. Declaration order is the catalog order shown in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardField {
    Cost,
    Tokens,
    Profile,
    Model,
    Effort,
    Harness,
    Stage,
    Progress,
    Rounds,
    Age,
    DependsOn,
    Priority,
    Artifacts,
    Pinned,
    Lines,
    NeedsMe,
}

impl CardField {
    pub const ALL: [CardField; 16] = [
        CardField::Cost,
        CardField::Tokens,
        CardField::Profile,
        CardField::Model,
        CardField::Effort,
        CardField::Harness,
        CardField::Stage,
        CardField::Progress,
        CardField::Rounds,
        CardField::Age,
        CardField::DependsOn,
        CardField::Priority,
        CardField::Artifacts,
        CardField::Pinned,
        CardField::Lines,
        CardField::NeedsMe,
    ];

    /// The name used in `fields = [...]` and `--columns`.
    pub fn key(self) -> &'static str {
        match self {
            CardField::Cost => "cost",
            CardField::Tokens => "tokens",
            CardField::Profile => "profile",
            CardField::Model => "model",
            CardField::Effort => "effort",
            CardField::Harness => "harness",
            CardField::Stage => "stage",
            CardField::Progress => "progress",
            CardField::Rounds => "rounds",
            CardField::Age => "age",
            CardField::DependsOn => "depends_on",
            CardField::Priority => "priority",
            CardField::Artifacts => "artifacts",
            CardField::Pinned => "pinned",
            CardField::Lines => "lines",
            CardField::NeedsMe => "needs_me",
        }
    }

    pub fn sample(self) -> &'static str {
        match self {
            CardField::Cost => "$1.42",
            CardField::Tokens => "48.2k tok",
            CardField::Profile => "sonnet",
            CardField::Model => "claude-sonnet-4",
            CardField::Effort => "high",
            CardField::Harness => "claude",
            CardField::Stage => "reviewing",
            CardField::Progress => "Build 2/4",
            CardField::Rounds => "r1",
            CardField::Age => "12m",
            CardField::DependsOn => "dep T-050",
            CardField::Priority => "P1",
            CardField::Artifacts => "◆ 3",
            CardField::Pinned => "★",
            CardField::Lines => "+120 −14",
            CardField::NeedsMe => "which API shape?",
        }
    }

    /// Drops the lowest-priority (last) parts until their ` · `-joined line fits `width` (the first part stays).
    pub fn fit<T>(mut parts: Vec<(T, String)>, width: usize) -> Vec<(T, String)> {
        let line_width = |parts: &[(T, String)]| {
            let text: usize = parts.iter().map(|(_, s)| s.chars().count()).sum();
            text + 3 * parts.len().saturating_sub(1)
        };
        while parts.len() > 1 && line_width(&parts) > width {
            parts.pop();
        }
        parts
    }

    pub fn parse(s: &str) -> Option<CardField> {
        CardField::ALL.into_iter().find(|f| f.key() == s.trim())
    }

    pub fn help(self) -> &'static str {
        match self {
            CardField::Cost => {
                "total spend across runs, reviews and gates ($ reported, sub ~$ API-equivalent)"
            }
            CardField::Tokens => "total tokens used",
            CardField::Profile => "profile of the current run",
            CardField::Model => "model of the current run",
            CardField::Effort => "effort level of the current run",
            CardField::Harness => "harness of the current run",
            CardField::Stage => "autopilot stage (gating, reviewing, changes 1/2, ...)",
            CardField::Progress => "checklist progress of the current run",
            CardField::Rounds => "change rounds so far",
            CardField::Age => "time since the run started (Running) or the task last changed",
            CardField::DependsOn => "unfinished dependencies",
            CardField::Priority => "priority",
            CardField::Artifacts => "artifact count of the current run",
            CardField::Pinned => "star when the task is pinned",
            CardField::Lines => "lines added and removed by the current run",
            CardField::NeedsMe => "why the task needs the owner",
        }
    }

    /// The field's text for `task`, or `None` when it has nothing to say (the field is then skipped).
    pub fn text(self, t: &Task, all: &[Task], now: Timestamp, pinned: bool) -> Option<String> {
        let run = t.current_run();
        match self {
            CardField::Cost => Some(cost_text(t)),
            CardField::Tokens => t
                .total_usage()
                .map(|u| format!("{} tok", short_count(u.total_tokens()))),
            CardField::Profile => run.map(|r| r.profile.clone().unwrap_or_else(|| r.agent.clone())),
            CardField::Model => run.and_then(|r| r.model.clone()),
            CardField::Effort => run.and_then(|r| r.effort.clone()),
            CardField::Harness => run.and_then(|r| r.harness.clone()),
            CardField::Stage => run
                .and_then(|r| r.autopilot.as_ref())
                .map(|a| match a.stage {
                    AutoStage::Gating => "gating".to_string(),
                    AutoStage::Reviewing => "reviewing".to_string(),
                    AutoStage::Landing => "landing".to_string(),
                    AutoStage::Done => "landed".to_string(),
                    AutoStage::Reviewed => "reviewed".to_string(),
                    AutoStage::Changes => format!("changes r{}", t.rounds),
                    AutoStage::Blocked => "blocked".to_string(),
                    AutoStage::Escalated => "escalated".to_string(),
                }),
            CardField::Progress => run.and_then(|r| r.progress.as_ref()).map(|p| p.summary()),
            CardField::Rounds => (t.rounds > 0).then(|| format!("r{}", t.rounds)),
            CardField::Age => {
                let since = match (t.status, run) {
                    (Status::Running, Some(r)) => r.started,
                    _ => t.updated,
                };
                Some(age_text((now.as_second() - since.as_second()).max(0)))
            }
            CardField::DependsOn => {
                let open: Vec<&str> = t
                    .depends_on
                    .iter()
                    .filter(|d| !all.iter().any(|x| &x.id == *d && x.status == Status::Done))
                    .map(String::as_str)
                    .collect();
                (!open.is_empty()).then(|| format!("dep {}", open.join(",")))
            }
            CardField::Priority => Some(t.priority.clone()),
            CardField::Artifacts => {
                run.map(|r| format!("◆ {}", t.artifacts.iter().filter(|a| a.run == r.id).count()))
            }
            CardField::Pinned => pinned.then(|| "★".to_string()),
            CardField::Lines => run
                .and_then(|r| diff_counts(t, &r.id))
                .map(|(a, d)| format!("{a} {d}")),
            CardField::NeedsMe => {
                if t.status != Status::NeedsMe {
                    return None;
                }
                let r = run?;
                r.obligations
                    .pending_decision()
                    .map(|d| d.request.question.clone())
                    .or_else(|| r.autopilot.as_ref().and_then(|a| a.reason.clone()))
                    .and_then(|s| {
                        s.lines()
                            .map(str::trim)
                            .find(|l| !l.is_empty())
                            .map(str::to_string)
                    })
            }
        }
    }
}

/// "+A −R" parsed from the run's "Final diff +A −R" artifact title.
pub fn diff_counts(task: &Task, run_id: &str) -> Option<(String, String)> {
    let a = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.run == run_id && a.kind == ArtifactKind::Diff)?;
    let mut words = a
        .title
        .split_whitespace()
        .filter(|w| w.starts_with('+') || w.starts_with('−') || w.starts_with('-'));
    Some((words.next()?.to_string(), words.next()?.to_string()))
}

/// `45s`, `12m`, `3h05m`, `2d`.
pub fn age_text(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d", s / 86400),
    }
}

/// `950`, `1.2k`, `3.4M`.
pub fn short_count(n: u64) -> String {
    let f = |n: u64, div: u64, unit: &str| {
        let tenths = n * 10 / div;
        format!("{}.{}{unit}", tenths / 10, tenths % 10)
    };
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => f(n, 1_000, "k"),
        _ => f(n, 1_000_000, "M"),
    }
}

/// A task's spend in USD across every run, review and gate, and whether it is harness-reported (`true`) or an
/// API-equivalent estimate (`false`). `None` when no usage is priced.
pub fn cost_value(t: &Task) -> Option<(f64, bool)> {
    let u = t.total_usage()?;
    match u.reported_usd.or(u.cost_usd) {
        Some(v) => Some((v, true)),
        None => u.estimated_usd.map(|v| (v, false)),
    }
}

fn dollars(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

pub fn cost_text(t: &Task) -> String {
    match cost_value(t) {
        Some((v, true)) => format!("${}", dollars(v)),
        Some((v, false)) => format!("sub ~${}", dollars(v)),
        None if t.runs.is_empty() => "$0.00".to_string(),
        None => "$—".to_string(),
    }
}

/// Sum of the priced spend of `tasks` (reported and estimated alike), for column headers.
pub fn cost_sum<'a>(tasks: impl IntoIterator<Item = &'a Task>) -> f64 {
    tasks
        .into_iter()
        .filter_map(cost_value)
        .fold(0.0, |sum, (v, _)| sum + v)
}

pub fn cost_sum_text(sum: f64) -> String {
    format!("${}", dollars(sum))
}

/// `[board]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BoardCfg {
    #[serde(default)]
    pub card: CardCfg,
}

impl BoardCfg {
    pub fn is_default(&self) -> bool {
        *self == BoardCfg::default()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// One line per card: id, title and the fields.
    Compact,
    #[default]
    Comfortable,
    /// Comfortable plus a progress bar line.
    Rich,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Compact, Density::Comfortable, Density::Rich];

    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Comfortable => "comfortable",
            Density::Rich => "rich",
        }
    }
}

/// `[board.card]`: which fields a board card shows. The list order is the priority: at narrow widths the last
/// fields are dropped first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardCfg {
    #[serde(default = "default_fields")]
    pub fields: Vec<String>,
    #[serde(default)]
    pub density: Density,
    /// Per-column override: status key (`running`, `needs_me`, ...) -> fields.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub columns: BTreeMap<String, Vec<String>>,
    /// Cost at or above this (USD) is drawn in the warning color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_warn: Option<f64>,
    /// Show the sum of the cards' cost in each column header.
    #[serde(default)]
    pub column_cost: bool,
}

fn default_fields() -> Vec<String> {
    vec!["cost".to_string()]
}

impl Default for CardCfg {
    fn default() -> Self {
        CardCfg {
            fields: default_fields(),
            density: Density::default(),
            columns: BTreeMap::new(),
            cost_warn: None,
            column_cost: false,
        }
    }
}

pub fn status_key(s: Status) -> &'static str {
    match s {
        Status::Idea => "idea",
        Status::Backlog => "backlog",
        Status::Ready => "ready",
        Status::Running => "running",
        Status::NeedsMe => "needs_me",
        Status::Review => "review",
        Status::Done => "done",
        Status::Dropped => "dropped",
    }
}

impl CardCfg {
    /// The configured fields for a card in `status`'s column, in priority order, unknown names skipped.
    pub fn fields_for(&self, status: Status) -> Vec<CardField> {
        let names = self.columns.get(status_key(status)).unwrap_or(&self.fields);
        names.iter().filter_map(|n| CardField::parse(n)).collect()
    }

    pub(crate) fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let list = |key: String, names: &[String], out: &mut Vec<String>| {
            for n in names {
                if CardField::parse(n).is_none() {
                    let known: Vec<&str> = CardField::ALL.iter().map(|f| f.key()).collect();
                    out.push(format!(
                        "{key}: unknown field '{n}' (available: {})",
                        known.join(", ")
                    ));
                }
            }
        };
        list("board.card.fields".into(), &self.fields, &mut out);
        for (col, names) in &self.columns {
            if !Status::COLUMNS.iter().any(|s| status_key(*s) == col) {
                out.push(format!("board.card.columns.{col}: unknown column"));
            }
            list(format!("board.card.columns.{col}"), names, &mut out);
        }
        if self.cost_warn.is_some_and(|w| !w.is_finite() || w <= 0.0) {
            out.push("board.card.cost_warn must be > 0".to_string());
        }
        out
    }
}

impl Config {
    /// The configured field list for a column, for callers that only have a `Config`.
    pub fn card_fields(&self, status: Status) -> Vec<CardField> {
        self.board.card.fields_for(status)
    }
}

#[cfg(test)]
#[path = "card_tests.rs"]
mod tests;

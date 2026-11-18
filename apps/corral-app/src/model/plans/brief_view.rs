//! Compact Plans content, independent of screen geometry and form.
use super::{Action, GraphInput, ListNotice};
use crate::model::Id;
use crate::model::board::Mark;
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;
use crate::model::text::Rich;

#[derive(Debug, Clone, PartialEq)]
pub struct PlanOutline {
    pub title: &'static str,
    pub stats: Vec<Stat>,
    pub notice: Option<ListNotice>,
    pub sections: Vec<OutlineSection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutlineSection {
    pub label: &'static str,
    pub hue: Hue,
    pub count: usize,
    pub rows: Vec<OutlineRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutlineRow {
    pub test_id: String,
    pub id: Id,
    pub id_label: String,
    pub title: String,
    /// The plan's project, when the scope spans several.
    pub project_chip: Option<String>,
    pub selected: bool,
    pub action: Action,
    pub second: OutlineSecond,
    /// The full title and what the second line stands for: the row's hint, and on the phone,
    /// which has no hover, the sentence its screen reader label ends with.
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutlineSecond {
    Members {
        /// One segment per member, from the row's per-status counts.
        segments: Vec<(Mark, Hue)>,
        /// Past `STRIP_EACH` members, each state's share of one bar instead of a segment per
        /// member: done, needs you, moving, waiting.
        shares: Option<Vec<MemberShare>>,
        progress: String,
        state: StateIndicator,
    },
    Status {
        text: String,
        hint: String,
        hue: Hue,
    },
}

/// One state icon, without a redundant visible state sentence or invented count.
#[derive(Debug, Clone, PartialEq)]
pub struct StateIndicator {
    pub icon: StatIcon,
    pub hue: Hue,
    pub hint: String,
}

/// How many members of a plan share one state, drawn as that share of the member bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberShare {
    pub hue: Hue,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanBrief {
    /// What holds the plan up, led in its hue, then how far it has come.
    pub headline: Rich,
    /// The dependency graph's members, each with its named rail.
    pub graph: GraphInput,
    pub path: PathToDone,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathToDone {
    pub title: &'static str,
    pub hint: &'static str,
    pub done: usize,
    pub total: usize,
    /// First unfinished step; none only when every step is done.
    pub current: Option<usize>,
    pub steps: Vec<PathStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathStep {
    pub mark: Mark,
    pub hue: Hue,
    pub title: String,
    pub hint: String,
    pub items: Vec<PathItem>,
    pub warning: Option<PathWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathItem {
    pub mark: Mark,
    pub hue: Hue,
    pub text: String,
    pub hint: String,
    pub action: Option<BriefAction>,
}

/// One of a Blocked task's served next steps, or another press attached to a fact.
#[derive(Debug, Clone, PartialEq)]
pub struct BriefAction {
    pub label: String,
    pub action: Action,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathWarning {
    pub text: String,
    pub hint: String,
    pub action: Option<BriefAction>,
}

//! Typed who-acts-next rows, including the phone-specific collapsed projection.

use crate::model::board::TaskLine;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::{Button, GroupKey, Press};

/// A board task row: the facts it draws, and how it reacts to selection and presses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    /// Id, title, actor, delta, rail, now, age and cost, resolved for the row.
    pub line: TaskLine,
    /// Whether the desktop list exposes this as its selected option.
    pub selected: bool,
    /// Select on desktop, push a task or question on phone.
    pub action: Press,
}

/// The rows a list renderer walks, in exactly their visible order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A normal board task.
    Task(TaskRow),
    /// The collapsed queued-task expansion line.
    More(Button),
}

/// A heading and the list rows beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    /// Stable group key.
    pub key: GroupKey,
    /// Group heading text.
    pub label: &'static str,
    /// Header count, including collapsed or omitted sample rows.
    pub count: u32,
    /// State hue of the heading.
    pub hue: Hue,
    /// The Corral heading's additional vocabulary, absent for other groups.
    pub detail: Option<&'static str>,
    /// Task, extra and summary rows in display order.
    pub rows: Vec<Row>,
}

/// The entire who-acts-next list, or its loading placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupedList {
    /// Accessible list label and desktop dock tab title.
    pub label: &'static str,
    /// Desktop column labels; phone rows use their two-line anatomy instead.
    pub columns: &'static [&'static str],
    /// Accessible description of the rail header.
    pub rail_title: &'static str,
    /// Placeholder row count; a nonzero count replaces all groups.
    pub loading_rows: usize,
    /// One quiet line instead of the groups: no board yet, or nothing on it.
    pub notice: Option<&'static str>,
    /// The six source groups, except the desktop Empty projection omits Needs you.
    pub groups: Vec<GroupView>,
}

impl GroupedList {
    /// Accessible list label and desktop dock tab title.
    pub const LABEL: &'static str = "Who acts next";

    /// The list's frame for a form, with no rows yet.
    pub(super) fn new(form: Form) -> GroupedList {
        let phone = form == Form::Phone;
        GroupedList {
            label: Self::LABEL,
            columns: if phone {
                &[]
            } else {
                &["id", "task", "r g v l", "now", "age", "cost"]
            },
            rail_title: "run · gate · review · land",
            loading_rows: 0,
            notice: None,
            groups: Vec::new(),
        }
    }

    /// Placeholder rows while the list's detail loads.
    pub(super) fn loading(form: Form) -> GroupedList {
        GroupedList {
            loading_rows: if form == Form::Phone { 6 } else { 12 },
            ..GroupedList::new(form)
        }
    }
}

/// One recorded count in the phone summary strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryChip {
    pub label: String,
    pub hue: Hue,
    /// Number carried by the label, supplied by the canonical feed.
    pub count: u32,
}

impl SummaryChip {
    pub fn new(label: impl Into<String>, hue: Hue, count: u32) -> Self {
        Self {
            label: label.into(),
            hue: if count == 0 { Hue::Neutral } else { hue },
            count,
        }
    }
}

/// The phone Now page projected from the shared feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneNow {
    pub title: &'static str,
    pub chips: Vec<SummaryChip>,
    pub summary_action: Option<super::Action>,
    pub list: GroupedList,
    pub status: super::Status,
}

//! Typed who-acts-next rows: the board's tasks grouped by state or by project, which Now's
//! sections, Waiting line and keys are built from.

use crate::model::board::TaskLine;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::{Button, GroupKey, Press};

/// A board task row: the facts it draws, and how it reacts to selection and presses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    /// Id, title, actor, delta, rail, now, age and cost, resolved for the row.
    pub line: TaskLine,
    /// Project context when the unfiltered workspace list needs disambiguation.
    pub project_chip: Option<String>,
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
    /// State bucket; project sections use Queued and are identified by their heading.
    pub key: GroupKey,
    /// Group heading text.
    pub label: String,
    /// Header count, including collapsed or omitted sample rows.
    pub count: u32,
    /// State hue of the heading.
    pub hue: Hue,
    /// Task, extra and summary rows in display order.
    pub rows: Vec<Row>,
}

/// The entire who-acts-next list, or its loading placeholders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupedList {
    /// Placeholder row count; a nonzero count replaces all groups.
    pub loading_rows: usize,
    /// One quiet line instead of the groups: no board yet, or nothing on it.
    pub notice: Option<&'static str>,
    /// State buckets, or alphabetical project sections with Unassigned last.
    pub groups: Vec<GroupView>,
}

impl GroupedList {
    /// One quiet line in place of the groups.
    pub(super) fn quiet(notice: &'static str) -> GroupedList {
        GroupedList {
            notice: Some(notice),
            ..GroupedList::default()
        }
    }

    /// Placeholder rows while the list's detail loads.
    pub(super) fn loading(form: Form) -> GroupedList {
        GroupedList {
            loading_rows: if form == Form::Phone { 6 } else { 12 },
            ..GroupedList::default()
        }
    }
}

//! Typed who-acts-next rows, including the phone-specific collapsed projection.

use crate::model::board::{money, task};
use crate::model::effect::{DataMode, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::{Action, Button, ButtonKind, Extra, ExtraKind, GROUPS, GroupKey, ParkedSummary, State};

/// A board task row; all common one-line/two-line facts come from `board::task(id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    /// The shared board record containing title, actor, delta, rail, now, age and cost.
    pub id: &'static str,
    /// Whether the desktop list exposes this as its selected option.
    pub selected: bool,
    /// Select on desktop, push a task or question on phone.
    pub action: Action,
    /// The phone's right-aligned second-line age/cost, with separators resolved.
    pub phone_meta: String,
}

/// A complete non-task row accompanying T-868.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraRow {
    /// Original data, including full id and illustrative flag.
    pub extra: Extra,
    /// Display id, with `PM/` removed.
    pub display_id: &'static str,
    /// `plan` or `PM ask`; hidden when the desktop question is no longer open.
    pub kind_label: &'static str,
    /// Plan notes are green only while the question is open, matching the desktop branch.
    pub note_hue: Hue,
    /// `10h` for the plan, `12m` for the PM question; blank in the answered desktop branch.
    pub age: &'static str,
    /// Whether the row carries a changed-since-visit triangle.
    pub changed: bool,
    /// Open a plan or show the sample notice.
    pub action: Action,
}

/// A parked aggregate with its model-resolved wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParkedRow {
    /// Counts, project split and explanatory note.
    pub summary: ParkedSummary,
    /// Main row wording: split alone on desktop, split plus held attribution on phone.
    pub title: String,
    /// Desktop now-phrase; empty on phone where it is part of the title.
    pub now: &'static str,
    /// The mockup's explanatory toast.
    pub action: Action,
}

/// The rows a list renderer walks, in exactly their visible order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A normal board task.
    Task(TaskRow),
    /// A plan accept or sample PM ask.
    Extra(ExtraRow),
    /// The collapsed queued-task expansion line.
    More(Button),
    /// The aggregate owner-held block.
    Parked(ParkedRow),
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
    /// The six source groups, except the desktop Empty projection omits Needs you.
    pub groups: Vec<GroupView>,
}

/// Whether the question is open according to the available environment projection.
/// An app with an answered/queued inbox state supplies that state through the explicit builder.
pub fn question_open(env: Env) -> bool {
    !matches!(env.mode, DataMode::Empty | DataMode::Elsewhere)
}

/// Selectable visible task ids, excluding extra plan/PM rows and collapsed summaries.
/// Phone queues expose four ids while desktop/terminal expose eight.
pub fn visible_task_ids(env: Env, question_is_open: bool) -> Vec<&'static str> {
    selectable_tasks(env, question_is_open).collect()
}

/// Iterate selectable rows without allocating during keyboard navigation.

pub(super) fn selectable_tasks(
    env: Env,
    question_is_open: bool,
) -> impl Iterator<Item = &'static str> {
    let phone = env.form == Form::Phone;
    GROUPS.iter().flat_map(move |group| {
        let limit = if phone && group.key == GroupKey::Queued {
            4
        } else {
            group.ids.len()
        };
        group.ids.iter().take(limit).copied().filter(move |_| {
            env.mode != DataMode::Loading
                && (group.key != GroupKey::You
                    || phone
                    || question_is_open && env.mode != DataMode::Empty)
        })
    })
}

/// Build the desktop/terminal or phone list using only the environment's question projection.
pub fn grouped_list(state: &State, env: Env) -> GroupedList {
    grouped_list_with_question(state, env, question_open(env))
}

/// Build the list with the inbox's question-open projection supplied by the app.
/// The phone deliberately retains its T-868 and plan rows even in Empty/Elsewhere modes,
/// as the source does. Desktop hides T-868 when the question is no longer open.
pub fn grouped_list_with_question(state: &State, env: Env, question_is_open: bool) -> GroupedList {
    let phone = env.form == Form::Phone;
    let mut view = GroupedList {
        label: "Who acts next",
        columns: if phone {
            &[]
        } else {
            &["id", "task", "r g v l", "now", "age", "cost"]
        },
        rail_title: "run · gate · review · land",
        loading_rows: if env.mode == DataMode::Loading {
            if phone { 6 } else { 12 }
        } else {
            0
        },
        groups: Vec::new(),
    };
    if view.loading_rows != 0 {
        return view;
    }
    for group in GROUPS {
        if !phone && env.mode == DataMode::Empty && group.key == GroupKey::You {
            continue;
        }
        let mut rows = Vec::new();
        if group.key == GroupKey::You {
            if phone || question_is_open {
                rows.push(Row::Task(task_row("T-868", state, phone, true)));
            }
            for extra in group
                .extra
                .iter()
                .filter(|extra| !phone || extra.kind == ExtraKind::Plan)
            {
                let active = phone || question_is_open;
                rows.push(Row::Extra(ExtraRow {
                    extra: *extra,
                    display_id: extra.id.strip_prefix("PM/").unwrap_or(extra.id),
                    kind_label: if !active {
                        ""
                    } else if extra.kind == ExtraKind::Plan {
                        "plan"
                    } else {
                        "PM ask"
                    },
                    note_hue: if active && extra.kind == ExtraKind::Plan {
                        Hue::Ok
                    } else {
                        Hue::Neutral
                    },
                    age: if !active {
                        ""
                    } else if extra.kind == ExtraKind::Plan {
                        "10h"
                    } else {
                        "12m"
                    },
                    changed: !phone && active && extra.kind == ExtraKind::Plan,
                    action: if extra.kind == ExtraKind::Plan || !active {
                        Action::OpenPlan(extra.id)
                    } else {
                        Action::SampleQuestion
                    },
                }));
            }
        } else {
            let limit = if phone && group.key == GroupKey::Queued {
                4
            } else {
                group.ids.len()
            };
            for id in group.ids.iter().take(limit) {
                rows.push(Row::Task(task_row(id, state, phone, false)));
            }
        }
        if group.more != 0 {
            rows.push(Row::More(Button::new(
                if phone {
                    "+ 19 more queued · after a dependency or capacity"
                } else {
                    "+ 15 more queued · capacity"
                },
                Action::MoreQueued,
                ButtonKind::Quiet,
            )));
        }
        if let Some(summary) = group.parked {
            rows.push(Row::Parked(ParkedRow {
                summary,
                title: if phone {
                    format!("{} · held by you", summary.split)
                } else {
                    summary.split.into()
                },
                now: if phone {
                    ""
                } else {
                    "held by you · not counted"
                },
                action: Action::ParkedSummary,
            }));
        }
        view.groups.push(GroupView {
            key: group.key,
            label: group.label,
            count: match group.key {
                GroupKey::Queued => 23,
                GroupKey::Parked => 32,
                _ => (group.ids.len() + group.extra.len()) as u32,
            },
            hue: group.key.hue(),
            detail: if !phone && group.key == GroupKey::Corral {
                Some("gating · reviewing · landing")
            } else {
                None
            },
            rows,
        });
    }
    view
}

fn task_row(id: &'static str, state: &State, phone: bool, question: bool) -> TaskRow {
    let task = task(id);
    let phone_meta = if !phone {
        String::new()
    } else {
        let cost = money(task.cost);
        match (task.age, cost.is_empty()) {
            (Some(age), false) => format!("{age} · {cost}"),
            (Some(age), true) => age.into(),
            (None, _) => cost,
        }
    };
    TaskRow {
        id,
        selected: !phone && state.sel == id,
        action: if phone && question {
            Action::OpenQuestion
        } else if phone {
            Action::OpenTask(id)
        } else {
            Action::Select(id)
        },
        phone_meta,
    }
}

/// A summary chip above the phone Now list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryChip {
    /// Complete chip wording.
    pub label: &'static str,
    /// State hue, neutral unless the chip needs the owner.
    pub hue: Hue,
    /// Number carried by the label.
    pub count: u32,
}

/// Phone Now content: a tappable changes summary followed by grouped two-line rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneNow {
    /// Large navigation title.
    pub title: &'static str,
    /// Summary chips in their source order.
    pub chips: &'static [SummaryChip],
    /// Pressing anywhere in the summary opens Changes.
    pub summary_action: Action,
    /// Groups with phone-specific omissions and +19 queued summary.
    pub list: GroupedList,
    /// Connection/stale projection shared with the workspace.
    pub status: super::Status,
}

/// Build the phone Now tab, independent of the caller's current presentation form.
pub fn phone_now(state: &State, mut env: Env) -> PhoneNow {
    env.form = Form::Phone;
    PhoneNow {
        title: "Now",
        chips: &[
            SummaryChip {
                label: "▲ 12 since 18:40",
                hue: Hue::Neutral,
                count: 12,
            },
            SummaryChip {
                label: "3 need you",
                hue: Hue::You,
                count: 3,
            },
            SummaryChip {
                label: "5 moving",
                hue: Hue::Neutral,
                count: 5,
            },
            SummaryChip {
                label: "23 queued",
                hue: Hue::Neutral,
                count: 23,
            },
        ],
        summary_action: Action::Go(crate::model::nav::Destination::Changes),
        list: grouped_list(state, env),
        status: super::status(env),
    }
}

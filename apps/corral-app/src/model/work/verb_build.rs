//! A composer's values become one engine operation here, field by field as `verb_forms` lists
//! them. Nothing here checks whether the engine will accept it.
use corral_types::{NewTask, Op, Priority, TaskEdit};

use super::verb_forms::status;
use super::{Record, Verb};

pub(super) fn op(
    record: &Record,
    verb: Verb,
    values: &[String],
    initial: &[String],
) -> Result<Op, String> {
    let value = |index: usize| values.get(index).map_or("", |value| value.trim());
    let optional = |index| (!value(index).is_empty()).then(|| value(index).to_owned());
    let words = |index: usize| values.get(index).cloned().unwrap_or_default();
    let id = match record {
        Record::Task(id) => id.to_string(),
        Record::Workspace if verb == Verb::NewTask => String::new(),
        Record::Workspace => return Err("Choose a task first.".into()),
    };
    Ok(match verb {
        Verb::Edit => Op::Edit {
            id,
            edit: edit(values, initial)?,
        },
        Verb::Comment => Op::Comment { id, text: words(0) },
        Verb::Move => Op::Move {
            id,
            status: status(value(0))?,
        },
        // An untouched default is left to the engine, which resolves the same profile and base
        // and also applies what the app cannot see (a routing rule's title pattern).
        Verb::Dispatch => {
            let chosen = |index: usize| {
                let now = value(index);
                (initial.get(index).map_or("", |was| was.trim()) != now && !now.is_empty())
                    .then(|| now.to_owned())
            };
            Op::Dispatch {
                id,
                profile: chosen(0),
                base: chosen(1),
            }
        }
        Verb::Reply => Op::Reply { id, text: words(0) },
        Verb::Done => Op::Done {
            id,
            summary: words(0),
        },
        Verb::Gate => Op::Gate { id },
        Verb::Review => Op::Review {
            id,
            profile: optional(0),
        },
        Verb::Verdict => Op::Verdict {
            id,
            approve: value(0) == "approve",
            notes: optional(1),
        },
        Verb::Changes => Op::Changes { id, text: words(0) },
        Verb::Approve => Op::Approve { id, onto: None },
        Verb::Retry => Op::Retry {
            id,
            profile: optional(0),
            fresh: value(1) == "yes",
            note: optional(2),
        },
        Verb::Stop => Op::Stop { id },
        Verb::Drop => Op::Drop { id },
        Verb::Reopen => Op::Reopen { id },
        Verb::NewTask => Op::New {
            task: NewTask {
                title: value(0).to_owned(),
                body: words(1),
                status: status(value(2))?,
                project: optional(3),
                priority: priority(value(4))?,
                kind: optional(5),
                size: optional(6),
                parent: optional(7),
                depends_on: ids(value(8)),
                base: optional(9),
                profile: None,
            },
        },
    })
}

fn priority(value: &str) -> Result<Priority, String> {
    match value {
        "P0" => Ok(Priority::P0),
        "P1" => Ok(Priority::P1),
        "P2" => Ok(Priority::P2),
        "P3" => Ok(Priority::P3),
        other => Err(format!("Unknown priority {other}.")),
    }
}

/// Task ids from `T-1, T-2`.
fn ids(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Only the fields the owner changed; an emptied optional field clears it.
fn edit(values: &[String], initial: &[String]) -> Result<TaskEdit, String> {
    let changed = |index: usize| {
        let now = values.get(index).map_or("", |value| value.trim());
        (initial.get(index).map_or("", |value| value.trim()) != now).then_some(now)
    };
    let cleared = |index| changed(index).map(|now: &str| (!now.is_empty()).then(|| now.to_owned()));
    let edit = TaskEdit {
        title: changed(0).map(str::to_owned),
        body: changed(1).map(|_| values.get(1).cloned().unwrap_or_default()),
        project: cleared(2),
        priority: changed(3).map(priority).transpose()?,
        kind: cleared(4),
        size: cleared(5),
        profile: cleared(6),
        base: cleared(7),
        parent: cleared(8),
        depends_on: changed(9).map(ids),
        ..TaskEdit::default()
    };
    if edit == TaskEdit::default() {
        return Err("Nothing changed.".into());
    }
    Ok(edit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;

    #[test]
    fn an_edit_sends_only_changed_fields_and_an_emptied_field_clears() {
        let initial: Vec<String> = ["Title", "Body", "web", "P2", "", "", "", "", "", ""]
            .map(String::from)
            .to_vec();
        let mut values = initial.clone();
        values[2] = String::new();
        values[3] = "P1".into();
        let Ok(Op::Edit { edit, .. }) =
            op(&Record::Task(Id::new("T-1")), Verb::Edit, &values, &initial)
        else {
            panic!("an edit");
        };
        assert_eq!(edit.title, None);
        assert_eq!(edit.project, Some(None));
        assert_eq!(edit.priority, Some(Priority::P1));
        assert!(
            op(
                &Record::Task(Id::new("T-1")),
                Verb::Edit,
                &initial,
                &initial
            )
            .is_err()
        );
    }

    #[test]
    fn a_capture_carries_every_field_and_a_dispatch_leaves_untouched_defaults_to_the_engine() {
        let values: Vec<String> = [
            "Zoom presets",
            "",
            "ready",
            "corral",
            "P1",
            "ui",
            "s",
            "T-1",
            "T-14, T-12",
            "release",
        ]
        .map(String::from)
        .to_vec();
        let Ok(Op::New { task }) = op(&Record::Workspace, Verb::NewTask, &values, &[]) else {
            panic!("a new task");
        };
        assert_eq!(
            (
                task.status,
                task.priority,
                task.size.as_deref(),
                task.base.as_deref()
            ),
            (
                corral_types::Status::Ready,
                Priority::P1,
                Some("s"),
                Some("release")
            )
        );
        assert_eq!(task.depends_on, ["T-14", "T-12"]);
        assert_eq!(task.parent.as_deref(), Some("T-1"));
        let initial: Vec<String> = ["omp-sol", "main"].map(String::from).to_vec();
        let mut chosen = initial.clone();
        chosen[0] = "codex-terra".into();
        let task = Record::Task(Id::new("T-12"));
        assert_eq!(
            op(&task, Verb::Dispatch, &chosen, &initial),
            Ok(Op::Dispatch {
                id: "T-12".into(),
                profile: Some("codex-terra".into()),
                base: None,
            })
        );
    }
}

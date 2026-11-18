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
        Verb::Dispatch => Op::Dispatch {
            id,
            profile: optional(0),
            base: optional(1),
        },
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
                project: optional(2),
                status: status(value(3))?,
                ..NewTask::default()
            },
        },
    })
}

/// Only the fields the owner changed; an emptied optional field clears it.
fn edit(values: &[String], initial: &[String]) -> Result<TaskEdit, String> {
    let changed = |index: usize| {
        let now = values.get(index).map_or("", |value| value.trim());
        (initial.get(index).map_or("", |value| value.trim()) != now).then_some(now)
    };
    let cleared = |index| changed(index).map(|now: &str| (!now.is_empty()).then(|| now.to_owned()));
    let priority = changed(3)
        .map(|now| match now {
            "P0" => Ok(Priority::P0),
            "P1" => Ok(Priority::P1),
            "P2" => Ok(Priority::P2),
            "P3" => Ok(Priority::P3),
            other => Err(format!("Unknown priority {other}.")),
        })
        .transpose()?;
    let edit = TaskEdit {
        title: changed(0).map(str::to_owned),
        body: changed(1).map(|_| values.get(1).cloned().unwrap_or_default()),
        project: cleared(2),
        priority,
        profile: cleared(4),
        base: cleared(5),
        parent: cleared(6),
        depends_on: changed(7).map(|now| {
            now.split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect()
        }),
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
        let initial: Vec<String> = ["Title", "Body", "web", "P2", "", "", "", ""]
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
}

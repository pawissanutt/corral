//! A composer's values become one engine operation here, field by field as `verb_forms` lists
//! them. Nothing here checks whether the engine will accept it.
use corral_types::{NewTask, Op, Priority, TaskDifficulty, TaskEdit, normalize_tags};

use super::verb_forms::status;
use super::{Record, Verb};
use crate::model::tasks::meta;

pub(super) fn op(
    record: &Record,
    verb: Verb,
    values: &[String],
    initial: &[String],
) -> Result<Op, String> {
    let value = |index: usize| values.get(index).map_or("", |value| value.trim());
    let optional = |index| (!value(index).is_empty()).then(|| value(index).to_owned());
    let words = |index: usize| values.get(index).cloned().unwrap_or_default();
    // A value the owner changed from the form's default; an untouched default is left to the
    // engine, which resolves the same profile, base or landing target and also applies what the
    // app cannot see (a routing rule's title pattern).
    let chosen = |index: usize| {
        let now = value(index);
        (initial.get(index).map_or("", |was| was.trim()) != now && !now.is_empty())
            .then(|| now.to_owned())
    };
    let id = match record {
        Record::Task(id) => id.to_string(),
        Record::Workspace if verb == Verb::NewTask => String::new(),
        Record::Workspace | Record::Session(_) => return Err("Choose a task first.".into()),
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
            profile: chosen(0),
            base: chosen(1),
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
        Verb::Approve => Op::Approve {
            id,
            onto: chosen(0),
        },
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
                difficulty: difficulty(value(7))?,
                tags: tags(value(8))?,
                parent: optional(9),
                depends_on: ids(value(10)),
                base: optional(11),
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

/// A difficulty field's value: empty leaves it unset.
fn difficulty(value: &str) -> Result<Option<TaskDifficulty>, String> {
    (!value.is_empty())
        .then(|| value.parse::<TaskDifficulty>())
        .transpose()
}

/// A tag field's value as the exact tag list the engine takes.
fn tags(value: &str) -> Result<Vec<String>, String> {
    normalize_tags(meta::split_tags(value))
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
        difficulty: changed(6).map(difficulty).transpose()?,
        tags: changed(7).map(tags).transpose()?,
        profile: cleared(8),
        base: cleared(9),
        parent: cleared(10),
        depends_on: changed(11).map(ids),
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
        let initial: Vec<String> = ["Title", "Body", "web", "P2", "", "", "", "", "", "", "", ""]
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

    /// The Edit form showed a Markdown body on one line: its Body is a multi-line field, an
    /// edit of the title leaves the body alone, and an edited body goes out with its line breaks
    /// byte for byte.
    #[test]
    fn a_multi_line_body_survives_an_edit_of_another_field() {
        let body = "## Goal\nHour, day and week presets.\n\n## Acceptance\n- The behaviour\r\n";
        let task = corral_types::Task {
            title: "Timeline zoom presets".into(),
            body: body.into(),
            ..corral_types::Task::default()
        };
        let fields = super::super::verb_forms::fields(
            Verb::Edit,
            Some(&task),
            &super::super::verb_context::FormContext::default(),
        );
        assert!(fields[1].multiline);
        let initial: Vec<String> = fields.into_iter().map(|field| field.value).collect();
        assert_eq!(initial[1], body);
        let record = Record::Task(Id::new("T-12"));
        let mut values = initial.clone();
        values[0] = "Timeline zoom presets, hour to week".into();
        let Ok(Op::Edit { edit, .. }) = op(&record, Verb::Edit, &values, &initial) else {
            panic!("an edit");
        };
        assert_eq!(edit.body, None);
        values[1] = format!("{body}- Keeps the scroll position\n");
        let Ok(Op::Edit { edit, .. }) = op(&record, Verb::Edit, &values, &initial) else {
            panic!("an edit");
        };
        assert_eq!(edit.body.as_deref(), Some(values[1].as_str()));
    }

    /// An edit sent the metadata it showed, so a size off the scale was rewritten and tags
    /// changed elsewhere were lost: untouched size, difficulty and tags stay out of the edit,
    /// and a changed or emptied one goes out exactly, empty clearing it.
    #[test]
    fn an_edit_keeps_untouched_metadata_and_sends_what_changed() {
        let task = corral_types::Task {
            title: "Zoom".into(),
            size: Some("huge".into()),
            difficulty: Some(TaskDifficulty::Hard),
            tags: vec!["bug".into(), "needs design".into()],
            ..corral_types::Task::default()
        };
        let fields = super::super::verb_forms::fields(
            Verb::Edit,
            Some(&task),
            &super::super::verb_context::FormContext::default(),
        );
        assert!(
            fields[5]
                .choices
                .iter()
                .any(|choice| choice.value == "huge")
        );
        assert!(fields[7].tags);
        let initial: Vec<String> = fields.into_iter().map(|field| field.value).collect();
        let record = Record::Task(Id::new("T-4"));
        let sent = |change: &dyn Fn(&mut Vec<String>)| {
            let mut values = initial.clone();
            change(&mut values);
            match op(&record, Verb::Edit, &values, &initial) {
                Ok(Op::Edit { edit, .. }) => edit,
                other => panic!("an edit, not {other:?}"),
            }
        };
        let title = sent(&|values| values[0] = "Zoom presets".into());
        assert_eq!(
            (title.size, title.difficulty, title.tags),
            (None, None, None)
        );
        let tags = sent(&|values| values[7] = meta::remove_tag(&values[7], "bug"));
        assert_eq!(tags.tags, Some(vec!["needs design".to_owned()]));
        assert_eq!(tags.size, None);
        let cleared = sent(&|values| {
            values[6].clear();
            values[7].clear();
        });
        assert_eq!(cleared.difficulty, Some(None));
        assert_eq!(cleared.tags, Some(Vec::new()));
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
            "hard",
            "bug\nNeeds design",
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
        assert_eq!(task.difficulty, Some(TaskDifficulty::Hard));
        assert_eq!(task.tags, ["bug", "Needs design"]);
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

    /// The Actions menu's Approve opens on the run's base: left as it is, the engine lands onto
    /// that base; another branch typed in goes out as `onto`.
    #[test]
    fn approving_onto_another_branch_sends_that_branch() {
        let task = Record::Task(Id::new("T-11"));
        let initial = vec!["main".to_owned()];
        let approve = |onto: &str| op(&task, Verb::Approve, &[onto.to_owned()], &initial);
        assert_eq!(
            approve("main"),
            Ok(Op::Approve {
                id: "T-11".into(),
                onto: None,
            })
        );
        assert_eq!(
            approve(" release/0.9 "),
            Ok(Op::Approve {
                id: "T-11".into(),
                onto: Some("release/0.9".into()),
            })
        );
    }
}

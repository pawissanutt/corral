//! The input an operation asks for before it runs, prefilled from the task as served, and the
//! composer that collects it. A refusal's or Blocked card's next step that still needs words
//! (a reply, a summary) opens the same composer with its own values in place.
use corral_types::{Next, Op, Status};

use super::{Record, Sent, State, Verb, VerbField};
use crate::model::Id;
use crate::model::act::Act;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbComposer {
    pub title: String,
    pub fields: Vec<VerbField>,
    pub error: Option<String>,
    pub submit_label: String,
    pub record: Record,
    pub verb: Verb,
    /// The values the composer opened with; an edit sends only what changed.
    pub initial: Vec<String>,
    pub force: bool,
    /// The press that opened it already was the one local confirmation.
    pub confirmed: bool,
}

const fn field(
    label: &'static str,
    placeholder: &'static str,
    required: bool,
    choices: &'static [&'static str],
) -> (&'static str, &'static str, bool, &'static [&'static str]) {
    (label, placeholder, required, choices)
}

/// Every status, as the Move picker lists them.
pub const STATUSES: [&str; 8] = [
    "idea", "backlog", "ready", "running", "blocked", "review", "done", "dropped",
];
const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];
const VERDICTS: [&str; 2] = ["approve", "changes"];
const YES_NO: [&str; 2] = ["no", "yes"];
const NEW_STATUSES: [&str; 3] = ["idea", "backlog", "ready"];

type Spec = (&'static str, &'static str, bool, &'static [&'static str]);

fn specs(verb: Verb) -> Vec<Spec> {
    match verb {
        Verb::Edit => vec![
            field("Title", "Task title", true, &[]),
            field("Body", "Markdown: Goal, Acceptance, Notes", false, &[]),
            field("Project", "Project id; empty clears it", false, &[]),
            field("Priority", "", true, &PRIORITIES),
            field(
                "Profile",
                "Pinned worker profile; empty clears it",
                false,
                &[],
            ),
            field("Base", "Branch to start from and land onto", false, &[]),
            field("Parent", "Parent task id", false, &[]),
            field("Depends on", "Task ids, comma separated", false, &[]),
        ],
        Verb::Comment => vec![field("Comment", "Your comment", true, &[])],
        Verb::Move => vec![field("Status", "", true, &STATUSES)],
        Verb::Dispatch => vec![
            field(
                "Profile",
                "Worker profile; empty uses the resolved default",
                false,
                &[],
            ),
            field(
                "Base",
                "Base branch; empty uses the resolved default",
                false,
                &[],
            ),
        ],
        Verb::Reply => vec![field("Reply", "What the worker should hear", true, &[])],
        Verb::Done => vec![field("Summary", "What changed", true, &[])],
        Verb::Review => vec![field(
            "Reviewer profile",
            "Empty uses the project's reviewer",
            false,
            &[],
        )],
        Verb::Verdict => vec![
            field("Verdict", "", true, &VERDICTS),
            field("Notes", "Reviewer notes", false, &[]),
        ],
        Verb::Changes => vec![field("Notes", "What to change", true, &[])],
        Verb::Retry => vec![
            field(
                "Profile",
                "Worker profile; empty keeps the default",
                false,
                &[],
            ),
            field("From base", "", true, &YES_NO),
            field("Note", "Sent to the new run", false, &[]),
        ],
        Verb::NewTask => vec![
            field("Title", "Task title", true, &[]),
            field("Body", "Task description", false, &[]),
            field("Project", "Optional project", false, &[]),
            field("Status", "", true, &NEW_STATUSES),
        ],
        Verb::Gate | Verb::Approve | Verb::Stop | Verb::Drop | Verb::Reopen => Vec::new(),
    }
}

/// The verb's fields, prefilled from the task as the feed serves it.
pub(super) fn fields(verb: Verb, task: Option<&corral_types::Task>) -> Vec<VerbField> {
    let values = defaults(verb, task);
    specs(verb)
        .into_iter()
        .zip(values)
        .map(
            |((label, placeholder, required, choices), value)| VerbField {
                label,
                placeholder,
                required,
                value,
                choices,
            },
        )
        .collect()
}

fn defaults(verb: Verb, task: Option<&corral_types::Task>) -> Vec<String> {
    let text = |value: Option<&String>| value.cloned().unwrap_or_default();
    match (verb, task) {
        (Verb::Edit, Some(task)) => vec![
            task.title.clone(),
            task.body.clone(),
            text(task.project.as_ref()),
            task.priority.as_str().into(),
            text(task.profile.as_ref()),
            text(task.base.as_ref()),
            text(task.parent.as_ref()),
            task.depends_on.join(", "),
        ],
        (Verb::Edit, None) => vec![String::new(); 3]
            .into_iter()
            .chain(["P2".to_owned()])
            .chain(vec![String::new(); 4])
            .collect(),
        (Verb::Move, task) => vec![task.map_or("backlog", |task| task.status.as_str()).into()],
        (Verb::Verdict, _) => vec!["approve".into(), String::new()],
        (Verb::Retry, _) => vec![String::new(), "no".into(), String::new()],
        (Verb::NewTask, _) => vec![
            String::new(),
            String::new(),
            String::new(),
            "backlog".into(),
        ],
        (verb, _) => vec![String::new(); specs(verb).len()],
    }
}

/// The verb and values of an operation with input, so a next step can open its composer.
fn next_values(op: &Op) -> Option<(Verb, Vec<String>)> {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    Some(match op {
        Op::Comment { text: words, .. } => (Verb::Comment, vec![words.clone()]),
        Op::Reply { text: words, .. } => (Verb::Reply, vec![words.clone()]),
        Op::Done { summary, .. } => (Verb::Done, vec![summary.clone()]),
        Op::Changes { text: words, .. } => (Verb::Changes, vec![words.clone()]),
        Op::Dispatch { profile, base, .. } => (Verb::Dispatch, vec![text(profile), text(base)]),
        Op::Review { profile, .. } => (Verb::Review, vec![text(profile)]),
        Op::Verdict { approve, notes, .. } => (
            Verb::Verdict,
            vec![VERDICTS[usize::from(!approve)].into(), text(notes)],
        ),
        Op::Retry {
            profile,
            fresh,
            note,
            ..
        } => (
            Verb::Retry,
            vec![
                text(profile),
                YES_NO[usize::from(*fresh)].into(),
                text(note),
            ],
        ),
        _ => return None,
    })
}

impl State {
    pub(super) fn start_verb(&mut self, record: Record, verb: Verb, feed: &Feed) -> Vec<Effect> {
        self.menu = None;
        let task = match &record {
            Record::Task(id) => feed.board.task(id).map(|task| task.task()),
            Record::Workspace => None,
        };
        let fields = fields(verb, task);
        if fields.is_empty() {
            return match super::verb_build::op(&record, verb, &[], &[]) {
                Ok(op) => self.send(&record, op, false, false),
                Err(reason) => vec![Effect::Toast(reason)],
            };
        }
        self.composer = Some(VerbComposer {
            title: verb.title(&record),
            initial: fields.iter().map(|field| field.value.clone()).collect(),
            fields,
            error: None,
            submit_label: verb.submit_label().into(),
            record,
            verb,
            force: false,
            confirmed: false,
        });
        Vec::new()
    }

    /// Runs a refusal's or Blocked card's next step: at once when it carries all it needs,
    /// else in its composer with its values in place. `confirmed` when the press itself was the
    /// one local confirmation (a refusal sheet's forced entry, which lists what it skips).
    pub fn next(&mut self, next: &Next, confirmed: bool) -> Vec<Effect> {
        self.menu = None;
        let record = crate::model::act::target(&next.op)
            .map_or(Record::Workspace, |id| Record::Task(Id::new(id)));
        if let Some((verb, values)) = next_values(&next.op) {
            let mut fields = fields(verb, None);
            for (field, value) in fields.iter_mut().zip(&values) {
                field.value = value.clone();
            }
            if fields
                .iter()
                .any(|field| field.required && field.value.trim().is_empty())
            {
                self.composer = Some(VerbComposer {
                    title: verb.title(&record),
                    initial: values,
                    fields,
                    error: None,
                    submit_label: crate::model::act::label(&next.op, next.force),
                    record,
                    verb,
                    force: next.force,
                    confirmed,
                });
                return Vec::new();
            }
        }
        self.send(&record, next.op.clone(), next.force, confirmed)
    }

    pub(super) fn submit_verb(&mut self, values: Vec<String>) -> Vec<Effect> {
        let Some(mut composer) = self.composer.take() else {
            return Vec::new();
        };
        let problem = if values.len() != composer.fields.len() {
            Some("The action's input fields changed; reopen the action.".to_owned())
        } else {
            composer
                .fields
                .iter()
                .zip(&values)
                .find(|(field, value)| field.required && value.trim().is_empty())
                .map(|(field, _)| format!("{} is required.", field.label))
        };
        for (field, value) in composer.fields.iter_mut().zip(&values) {
            field.value = value.clone();
        }
        let op = match problem {
            Some(problem) => Err(problem),
            None => {
                super::verb_build::op(&composer.record, composer.verb, &values, &composer.initial)
            }
        };
        match op {
            Ok(op) => self.send(&composer.record, op, composer.force, composer.confirmed),
            Err(error) => {
                composer.error = Some(error);
                self.composer = Some(composer);
                Vec::new()
            }
        }
    }

    fn send(&mut self, record: &Record, op: Op, force: bool, confirmed: bool) -> Vec<Effect> {
        let mut act = Act::new(&mut self.ids, op, force);
        act.confirmed = confirmed;
        if let Record::Task(id) = record {
            self.sent.retain(|sent| sent.task != *id);
            self.sent.push(Sent {
                task: id.clone(),
                key: act.key.clone(),
                title: act.title.clone(),
            });
            if self.sent.len() > super::SENT_KEPT {
                self.sent.remove(0);
            }
        }
        vec![Effect::Act(act)]
    }
}

/// Parses a status the Move picker offered.
pub(super) fn status(value: &str) -> Result<Status, String> {
    Status::parse(value.trim()).map_err(|error| error.to_string())
}

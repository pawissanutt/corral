//! The input an operation asks for before it runs, prefilled from the task as served, and the
//! composer that collects it. A refusal's or Blocked card's next step that still needs words
//! (a reply, a summary) opens the same composer with its own values in place.
use corral_types::{Next, Op, Status, Task};

use super::verb_context::FormContext;
use super::{Choice, Record, Sent, State, Verb, VerbField};
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

/// Every status, as the Move picker lists them.
pub const STATUSES: [&str; 8] = [
    "idea", "backlog", "ready", "running", "blocked", "review", "done", "dropped",
];
const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];
const VERDICTS: [&str; 2] = ["approve", "changes"];
const YES_NO: [&str; 2] = ["no", "yes"];
/// Where a new task starts: an Idea to think about, the Backlog, or Ready to dispatch.
const NEW_STATUSES: [&str; 3] = ["idea", "backlog", "ready"];
/// The sizes routing rules name; empty leaves the task unsized.
const SIZES: [&str; 6] = ["", "xs", "s", "m", "l", "xl"];

/// A free-text field.
fn text(label: &'static str, placeholder: &'static str, value: impl Into<String>) -> VerbField {
    VerbField {
        label,
        placeholder,
        required: false,
        value: value.into(),
        choices: Vec::new(),
        suggestions: Vec::new(),
    }
}

/// A field that must not be left empty.
fn required(label: &'static str, placeholder: &'static str, value: impl Into<String>) -> VerbField {
    VerbField {
        required: true,
        ..text(label, placeholder, value)
    }
}

/// A closed set of values, one button each, `value` chosen; an empty value is a real choice
/// ("none", "Routing decides"), so a choice field is never missing.
fn pick(label: &'static str, values: &[&str], value: impl Into<String>) -> VerbField {
    VerbField {
        choices: values
            .iter()
            .map(|value| Choice {
                value: (*value).to_owned(),
                label: if value.is_empty() { "none" } else { value }.to_owned(),
            })
            .collect(),
        ..text(label, "", value)
    }
}

/// The project: one button per configured project and None, else free text.
fn project(ctx: &FormContext, value: Option<&String>) -> VerbField {
    let value = value.cloned().unwrap_or_default();
    if ctx.projects.is_empty() {
        return text("Project", "Project id; empty for none", value);
    }
    let mut ids = vec![String::new()];
    ids.extend(ctx.projects.iter().cloned());
    if !ids.contains(&value) {
        ids.push(value.clone());
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    pick("Project", &ids, value.clone())
}

/// The worker profile a dispatch runs: every configured profile, the one the engine would pick
/// marked and chosen. When the app cannot tell which, the first choice leaves it to the engine.
fn profile(ctx: &FormContext) -> VerbField {
    if ctx.profiles.is_empty() {
        return text(
            "Profile",
            "Worker profile; empty uses the resolved default",
            ctx.profile.clone().unwrap_or_default(),
        );
    }
    let mut names = ctx.profiles.clone();
    if let Some(chosen) = &ctx.profile
        && !names.contains(chosen)
    {
        names.push(chosen.clone());
    }
    let mut choices: Vec<Choice> = names
        .into_iter()
        .map(|name| Choice {
            label: if ctx.profile.as_ref() == Some(&name) {
                format!("{name} · default")
            } else {
                name.clone()
            },
            value: name,
        })
        .collect();
    if ctx.profile.is_none() {
        choices.insert(
            0,
            Choice {
                value: String::new(),
                label: "Routing decides".into(),
            },
        );
    }
    VerbField {
        choices,
        ..text("Profile", "", ctx.profile.clone().unwrap_or_default())
    }
}

/// The verb's fields, prefilled from the task as the feed serves it and offering the choices
/// the served configuration names.
pub(super) fn fields(verb: Verb, task: Option<&Task>, ctx: &FormContext) -> Vec<VerbField> {
    let of = |value: Option<&String>| value.cloned().unwrap_or_default();
    match verb {
        Verb::Edit => vec![
            required(
                "Title",
                "Task title",
                task.map(|task| task.title.clone()).unwrap_or_default(),
            ),
            text(
                "Body",
                "Markdown: Goal, Acceptance, Notes",
                task.map(|task| task.body.clone()).unwrap_or_default(),
            ),
            project(ctx, task.and_then(|task| task.project.as_ref())),
            pick(
                "Priority",
                &PRIORITIES,
                task.map_or("P2", |task| task.priority.as_str()),
            ),
            text(
                "Kind",
                "ui, bug, docs…; empty clears it",
                of(task.and_then(|task| task.kind.as_ref())),
            ),
            pick("Size", &SIZES, of(task.and_then(|task| task.size.as_ref()))),
            text(
                "Profile",
                "Pinned worker profile; empty clears it",
                of(task.and_then(|task| task.profile.as_ref())),
            ),
            text(
                "Base",
                "Branch to start from and land onto",
                of(task.and_then(|task| task.base.as_ref())),
            ),
            text(
                "Parent",
                "Parent task id",
                of(task.and_then(|task| task.parent.as_ref())),
            ),
            text(
                "Depends on",
                "Task ids, comma separated",
                task.map(|task| task.depends_on.join(", "))
                    .unwrap_or_default(),
            ),
        ],
        Verb::Comment => vec![required("Comment", "Your comment", "")],
        Verb::Move => vec![pick(
            "Status",
            &STATUSES,
            task.map_or("backlog", |task| task.status.as_str()),
        )],
        Verb::Dispatch => vec![
            profile(ctx),
            text(
                "Base",
                "Repository default branch",
                ctx.base.clone().unwrap_or_default(),
            ),
        ],
        Verb::Reply => vec![required("Reply", "What the worker should hear", "")],
        Verb::Done => vec![required("Summary", "What changed", "")],
        Verb::Review => vec![text(
            "Reviewer profile",
            "Empty uses the project's reviewer",
            "",
        )],
        Verb::Verdict => vec![
            pick("Verdict", &VERDICTS, "approve"),
            text("Notes", "Reviewer notes", ""),
        ],
        Verb::Changes => vec![required("Notes", "What to change", "")],
        // The run's base, which the engine lands onto unless another existing branch is named;
        // the project's local branches are offered beside it.
        Verb::Approve => vec![VerbField {
            suggestions: ctx.branches.clone(),
            ..text(
                "Onto",
                "Branch to land onto; empty lands onto the run's base",
                ctx.landing.clone().unwrap_or_default(),
            )
        }],
        Verb::Retry => vec![
            text("Profile", "Worker profile; empty keeps the default", ""),
            pick("From base", &YES_NO, "no"),
            text("Note", "Sent to the new run", ""),
        ],
        Verb::NewTask => vec![
            required("Title", "Task title", ""),
            text("Body", "Markdown: Goal, Acceptance, Notes", ""),
            pick("Status", &NEW_STATUSES, "backlog"),
            project(ctx, None),
            pick("Priority", &PRIORITIES, "P2"),
            text("Kind", "ui, bug, docs…", ""),
            pick("Size", &SIZES, ""),
            text("Parent", "Parent task id, for a plan's child", ""),
            text("Depends on", "Task ids, comma separated", ""),
            text("Base", "Empty uses the project's base", ""),
        ],
        Verb::Gate | Verb::Stop | Verb::Drop | Verb::Reopen => Vec::new(),
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
            Record::Workspace | Record::Session(_) => None,
        };
        let fields = fields(verb, task, &FormContext::new(feed, task));
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
            let mut fields = fields(verb, None, &FormContext::default());
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

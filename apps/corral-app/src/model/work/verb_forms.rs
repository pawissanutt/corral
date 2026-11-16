//! Structured composers retain the exact record/candidate the owner started acting on.
use super::{
    Record, Sent, State, Verb, VerbField,
    verbs::{Frozen, unavailable},
};
use crate::model::act::{self, Act};
use crate::model::daemon::Feed;
use crate::model::effect::Effect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbComposer {
    pub title: String,
    pub fields: Vec<VerbField>,
    pub error: Option<String>,
    pub submit_label: String,
    at: Frozen,
    verb: Verb,
}
fn field(label: &'static str, placeholder: &'static str, required: bool, value: &str) -> VerbField {
    VerbField {
        label,
        placeholder,
        required,
        value: value.into(),
    }
}
pub(super) fn fields(verb: Verb) -> Vec<VerbField> {
    match verb {
        Verb::Comment => vec![field("Comment", "Your comment", true, "")],
        Verb::Park => vec![
            field("Reason", "Why this task should wait", true, ""),
            field(
                "Resume conditions",
                "One check per line: landed:T-1 or status:T-1=done",
                false,
                "",
            ),
            field(
                "Resume action",
                "What to do when conditions pass",
                false,
                "",
            ),
            field("Review after (seconds)", "3600–604800", true, "86400"),
        ],
        Verb::NewTask => vec![
            field("Title", "Task title", true, ""),
            field("Body", "Task description", false, ""),
            field("Project", "Optional project", false, ""),
        ],
        Verb::PlanCancel => vec![field("Reason", "Why cancel this plan", false, "")],
        Verb::Verdict => vec![
            field("Verdict", "approve, changes or escalate", true, "approve"),
            field("Notes", "Candidate-bound verdict notes", false, ""),
        ],
        Verb::AddMember => vec![field("Member", "Task ID", true, "")],
        Verb::MoveMember => vec![
            field("Member", "Task ID", true, ""),
            field("Destination tracker", "Tracker ID", true, ""),
            field("Reason", "Why move this member", false, ""),
            field("Gate report path", "Optional report source path", false, ""),
            field(
                "Gate report contents",
                "Complete canonical gate report when required",
                false,
                "",
            ),
        ],
        Verb::Escalate => vec![
            field("Note", "Escalation context", false, ""),
            field(
                "Kind",
                "clarification, technical_choice, permission or owner_only",
                false,
                "",
            ),
        ],
        Verb::CancelQuestion => vec![field("Reason", "Why cancel this question", true, "")],
        Verb::Deadline => vec![
            field("Deadline", "RFC 3339 timestamp", true, ""),
            field("Instruction", "What to do at the deadline", true, ""),
        ],
        Verb::Route => vec![
            field("Worker profile", "Optional profile", false, ""),
            field("Reviewer profile", "Optional reviewer", false, ""),
            field("Clear reviewer", "yes or no", false, "no"),
            field("Model", "Optional model", false, ""),
            field("Effort", "Optional effort", false, ""),
        ],
        Verb::Move => vec![
            field("Status", "Canonical task status", true, "backlog"),
            field("Reason", "Why change status", false, ""),
        ],
        Verb::PermissionsRevoke => vec![field(
            "Grant",
            "Grant ID; leave empty to revoke all",
            false,
            "",
        )],
        _ => Vec::new(),
    }
}

impl State {
    fn uncertain(&self, feed: &Feed, record: &Record) -> Option<String> {
        self.sent_for(record.id())
            .filter(|sent| act::status(feed, &sent.key).is_some_and(|status| status.uncertain()))
            .map(|sent| {
                format!(
                    "{} has no known result yet; nothing new was sent.",
                    sent.title
                )
            })
    }
    pub(super) fn start_verb(&mut self, record: Record, verb: Verb, feed: &Feed) -> Vec<Effect> {
        let captured = self
            .uncertain(feed, &record)
            .map_or_else(|| Frozen::capture(feed, &record), Err);
        let at = match captured {
            Ok(at) => at,
            Err(reason) => return vec![Effect::Toast(reason)],
        };
        if let Some(reason) = unavailable(feed, &record, verb) {
            return vec![Effect::Toast(reason)];
        }
        self.menu = None;
        let fields = fields(verb);
        if fields.is_empty() {
            return match super::verb_build::command(&at, verb, &[]) {
                Ok(command) => match super::verbs::authority_reason(feed, command.app_class()) {
                    Some(reason) => vec![Effect::Toast(reason)],
                    None => self.send_verb(at, verb, command),
                },
                Err(reason) => vec![Effect::Toast(reason)],
            };
        }
        self.composer = Some(VerbComposer {
            title: verb.title(&record),
            fields,
            error: None,
            submit_label: verb.label().into(),
            at,
            verb,
        });
        Vec::new()
    }
    pub(super) fn submit_verb(&mut self, values: Vec<String>, feed: &Feed) -> Vec<Effect> {
        let Some(mut composer) = self.composer.take() else {
            return Vec::new();
        };
        let valid_count = values.len() == composer.fields.len();
        for (field, value) in composer.fields.iter_mut().zip(&values) {
            field.value = value.clone();
        }
        let problem = if !valid_count {
            Some("The action's input fields changed; reopen the action.".into())
        } else {
            composer
                .fields
                .iter()
                .find(|field| field.required && field.value.trim().is_empty())
                .map(|field| format!("{} is required.", field.label))
        };
        let problem = problem.or_else(|| self.uncertain(feed, &composer.at.record)).or_else(|| {
            match Frozen::capture(feed, &composer.at.record) {
                Ok(now) if now == composer.at => unavailable(feed, &composer.at.record, composer.verb),
                Ok(_) => Some("The record changed while you were writing. Reopen the action; your text is kept here.".into()),
                Err(reason) => Some(reason),
            }
        });
        if let Some(problem) = problem {
            composer.error = Some(problem);
            self.composer = Some(composer);
            return Vec::new();
        }
        let command =
            super::verb_build::command(&composer.at, composer.verb, &values).and_then(|command| {
                match super::verbs::authority_reason(feed, command.app_class()) {
                    Some(reason) => Err(reason),
                    None => Ok(command),
                }
            });
        match command {
            Ok(command) => self.send_verb(composer.at, composer.verb, command),
            Err(error) => {
                composer.error = Some(error);
                self.composer = Some(composer);
                Vec::new()
            }
        }
    }
    fn send_verb(
        &mut self,
        at: Frozen,
        verb: Verb,
        command: corral_protocol::cmd::Command,
    ) -> Vec<Effect> {
        let mut act = Act::new(&mut self.ids, verb.title(&at.record), command);
        if verb.confirms() {
            act = act.confirmed();
        }
        if let Some(expect) = at.expect() {
            act = act.expect(expect);
        }
        if let Some(live) = at.record.live() {
            act = act.about(live);
        }
        self.sent.retain(|sent| sent.task != at.record.id());
        self.sent.push(Sent {
            task: at.record.id().into(),
            key: act.key.clone(),
            title: act.title.clone(),
        });
        if self.sent.len() > super::SENT_KEPT {
            self.sent.remove(0);
        }
        vec![Effect::Act(act)]
    }
}

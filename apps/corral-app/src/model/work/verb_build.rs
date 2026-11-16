//! Frozen record identities and structured owner input become canonical commands here.
use super::{Verb, verbs::Frozen};
use corral_protocol::cmd::{
    Command, assistant::AssistantCommand as Assistant, attention::AttentionCommand as Attention,
    mode::ModeCommand as Mode, plan::PlanCommand as Plan, question::QuestionCommand as Question,
    queue::QueueCommand as Queue, review::ReviewCommand as Review, task::TaskCommand as Task,
};

pub(super) fn command(at: &Frozen, verb: Verb, values: &[String]) -> Result<Command, String> {
    let value = |index: usize| values.get(index).map(String::as_str).unwrap_or_default();
    let optional = |index| (!value(index).trim().is_empty()).then(|| value(index).to_owned());
    let id = at.record.id().to_owned();
    if verb == Verb::Route && !matches!(value(2).trim(), "" | "yes" | "no") {
        return Err("Clear reviewer must be yes or no.".into());
    }
    if verb == Verb::MoveMember && (value(3).is_empty() != value(4).is_empty()) {
        return Err("A gate report needs both its path and its complete contents.".into());
    }
    Ok(match verb {
        Verb::Hold => Command::Task(Task::Hold { id, reason: None }),
        Verb::Release => Command::Task(Task::Release { id }),
        Verb::Retry => Command::Task(Task::Retry {
            id,
            run: at.run.clone(),
            profile: None,
        }),
        Verb::Dispatch => Command::Task(Task::Dispatch {
            id,
            profile: None,
            model: None,
            effort: None,
            base: None,
            headless: true,
        }),
        Verb::QueueAdd => Command::Queue(Queue::Add {
            id,
            profile: None,
            model: None,
            effort: None,
            base: None,
            headless: true,
        }),
        Verb::QueueRemove => Command::Queue(Queue::Remove { id }),
        Verb::Park => Command::Attention(Attention::Park {
            id,
            reason: value(0).into(),
            resume_when: value(1)
                .lines()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_owned())
                .collect(),
            resume_action: value(2).into(),
            review_by: value(3)
                .parse()
                .map_err(|_| "Review interval must be seconds (3600–604800).")?,
            json: false,
        }),
        Verb::Unpark => Command::Attention(Attention::Unpark {
            id,
            expected_revision: at
                .park_revision
                .ok_or("The park revision is unavailable.")?,
            json: false,
        }),
        Verb::Comment => Command::Task(Task::Comment {
            id,
            text: value(0).into(),
            author: "owner".into(),
            send: true,
        }),
        Verb::NewTask => Command::Task(Task::New {
            title: vec![value(0).into()],
            origin: "owner".into(),
            project: optional(2),
            status: "backlog".into(),
            body: value(1).into(),
            profile: None,
            depends: Vec::new(),
            priority: None,
            kind: None,
            size: None,
            difficulty: None,
            design: false,
            labels: Vec::new(),
        }),
        Verb::Unstick => Command::Task(Task::Unstick { id }),
        Verb::Revert => Command::Task(Task::Revert { id }),
        Verb::Discard => Command::Review(Review::Discard { id }),
        Verb::PlanCancel => Command::Plan(Plan::Cancel {
            id,
            owner_ok: true,
            reason: optional(0),
            json: false,
        }),
        Verb::Verdict => Command::Review(Review::Verdict {
            id,
            sha: at.sha.clone(),
            verdict: value(0).trim().into(),
            notes: optional(1),
        }),
        Verb::AddMember => Command::Plan(Plan::AddMember {
            tracker: id,
            member: value(0).trim().into(),
            json: false,
        }),
        Verb::Escalate => Command::Question(Question::Escalate {
            key: id,
            rev: at.question_rev,
            note: optional(0),
            kind: optional(1),
            json: false,
        }),
        Verb::CancelQuestion => Command::Question(Question::Cancel {
            key: id,
            rev: at.question_rev,
            reason: value(0).into(),
            owner: true,
            json: false,
        }),
        Verb::MoveMember => Command::Plan(Plan::MoveMember {
            tracker: id,
            member: value(0).trim().into(),
            to: value(1).trim().into(),
            gate_file: (!value(3).is_empty()).then(|| corral_protocol::cmd::FileInput {
                path: value(3).into(),
                text: value(4).into(),
            }),
            reason: optional(2),
            json: false,
        }),
        Verb::Deadline => Command::Question(Question::Deadline {
            key: id,
            rev: at.question_rev,
            at: value(0).trim().into(),
            instruction: value(1).into(),
            json: false,
        }),
        Verb::Route => Command::Task(Task::Route {
            id,
            set_reviewer: optional(1),
            clear_reviewer: value(2).trim() == "yes",
            profile: optional(0),
            model: optional(3),
            effort: optional(4),
            json: false,
        }),
        Verb::Focus => Command::Task(Task::Focus {
            id: Some(id),
            all: false,
        }),
        Verb::Move => Command::Task(Task::Move {
            id,
            status: value(0).trim().into(),
            force: false,
            owner_ok: true,
            reason: optional(1),
        }),
        Verb::ModeOff => Command::Mode(Mode::Off { json: false }),
        Verb::PermissionsRevoke => Command::Assistant(Assistant::PermissionsRevoke {
            id,
            grant: optional(0),
            all: optional(0).is_none(),
            json: false,
        }),
    })
}

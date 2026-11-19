//! What Ask adds to the session composer it shares with the session page: the record it is
//! about and the default filing project as removable chips, whose ids travel as the message's
//! first line (`session.send` has no field for them, and the agent is the one who reads them),
//! and the staged attachments with the controls that stage them. And Ask without a connection.

use corral_protocol::assistant::ContextRef;

use super::views::context_chip;
use super::{Action, Composer, Staged, tray};
use crate::model::daemon::Feed;
use crate::model::id::Id;

/// The chips' words: the record's kind and id with the task's title when the board has it.
fn context_label(feed: &Feed, context: &ContextRef) -> String {
    match feed.board.task(&context.id) {
        Some(task) if context.kind == "task" => format!("{} {}", context.id, task.title()),
        _ => context.id.clone(),
    }
}

/// Ask's additions for the draft `text`, the `staged` attachments, the record `context` and
/// the default `project`.
pub fn composer(
    feed: &Feed,
    text: &str,
    staged: &[Staged],
    context: Option<&ContextRef>,
    project: Option<&Id>,
) -> Composer {
    let tray = tray(feed, staged, text.trim().is_empty());
    let holds_send = tray.holds_send.then(|| {
        tray.status
            .as_ref()
            .map(|line| line.iter().map(|span| span.text.as_str()).collect())
            .unwrap_or_else(|| "Attachments are not ready yet.".into())
    });
    Composer {
        chip: context
            .map(|context| context_chip(context.kind.clone(), context_label(feed, context))),
        project_chip: project.map(|id| {
            let mut chip = context_chip("project".into(), id.to_string());
            chip.note = "default project for filed ideas";
            chip.remove.action = Action::UnchipProject;
            chip.remove.label = "Remove project";
            chip
        }),
        status: tray.status,
        attachments: tray.buttons,
        staged: tray.chips,
        attach_note: tray.note,
        holds_send,
    }
}

/// The line the chips put before the owner's words: `Context: task T-12 · project site`.
fn context_line(context: Option<&ContextRef>, project: Option<&Id>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(context) = context {
        parts.push(format!("{} {}", context.kind, context.id));
    }
    if let Some(project) = project {
        parts.push(format!("project {project}"));
    }
    (!parts.is_empty()).then(|| format!("Context: {}", parts.join(" · ")))
}

/// The words Ask sends: the chips' line, then the owner's words. A draft that already starts
/// with that line (a refused send put back) is sent as it stands.
pub fn with_context(text: &str, context: Option<&ContextRef>, project: Option<&Id>) -> String {
    match context_line(context, project) {
        Some(line) if !text.trim_start().starts_with(&line) => format!("{line}\n\n{}", text.trim()),
        _ => text.trim().to_owned(),
    }
}

/// Ask without a connection to corrald: why nothing is sent, and the way to connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unconnected {
    pub title: &'static str,
    pub body: &'static str,
    pub connect: &'static str,
    pub send: &'static str,
    pub placeholder: &'static str,
}

pub const UNCONNECTED: Unconnected = Unconnected {
    title: "Not connected",
    body: "Connect this device to corrald to ask. Nothing is sent until it is paired.",
    connect: "Connect",
    send: "Send",
    placeholder: "Ask a question, drop an idea, or give an instruction…",
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused send puts its words back with the chips' line already in them; sending them
    /// again must not stack a second line on top.
    #[test]
    fn a_resent_draft_keeps_one_context_line() {
        let context = ContextRef {
            kind: "task".into(),
            id: "T-12".into(),
            rev: None,
        };
        let project = Id::from("site");
        let first = with_context("what blocks it?", Some(&context), Some(&project));
        assert_eq!(
            first,
            "Context: task T-12 · project site\n\nwhat blocks it?"
        );
        assert_eq!(with_context(&first, Some(&context), Some(&project)), first);
        assert_eq!(with_context(" plain ", None, None), "plain");
    }
}

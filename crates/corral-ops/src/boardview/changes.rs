//! The one event-to-owner-change vocabulary table. Unknown kinds remain `Other`;
//! consumers may hide them by default but must not discard their durable cursor.
use corral_core::backend::StoredEvent;
use corral_protocol::board::BoardGroup;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    Gate,
    Review,
    Land,
    Work,
    Attention,
    Question,
    Comment,
    Annotation,
    Automation,
    Policy,
    Queue,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct EventMapping {
    /// A trailing `*` matches all kinds with this prefix.
    pub kind: &'static str,
    pub class: ChangeClass,
    pub template: &'static str,
    /// The group whose state is affected, not a prediction of the resulting row.
    pub group: Option<BoardGroup>,
}

pub const EVENT_MAPPINGS: &[EventMapping] = &[
    EventMapping {
        kind: "gate_started",
        class: ChangeClass::Gate,
        template: "{task} · gating",
        group: Some(BoardGroup::Verifying),
    },
    EventMapping {
        kind: "gate",
        class: ChangeClass::Gate,
        template: "{task} · gate {result}",
        group: Some(BoardGroup::Verifying),
    },
    EventMapping {
        kind: "verdict",
        class: ChangeClass::Review,
        template: "{task} · review {verdict}",
        group: Some(BoardGroup::Verifying),
    },
    EventMapping {
        kind: "approved",
        class: ChangeClass::Land,
        template: "{task} · landed",
        group: Some(BoardGroup::Verifying),
    },
    EventMapping {
        kind: "done",
        class: ChangeClass::Work,
        template: "{task} · worker finished",
        group: Some(BoardGroup::Producing),
    },
    EventMapping {
        kind: "needs_me",
        class: ChangeClass::Attention,
        template: "{task} · needs you",
        group: Some(BoardGroup::NeedsYou),
    },
    EventMapping {
        kind: "stuck",
        class: ChangeClass::Attention,
        template: "{task} · worker stalled",
        group: Some(BoardGroup::Producing),
    },
    EventMapping {
        kind: "blocked",
        class: ChangeClass::Attention,
        template: "{task} · needs attention",
        group: Some(BoardGroup::NeedsYou),
    },
    EventMapping {
        kind: "escalated",
        class: ChangeClass::Attention,
        template: "{task} · needs you",
        group: Some(BoardGroup::NeedsYou),
    },
    EventMapping {
        kind: "question_*",
        class: ChangeClass::Question,
        template: "{task} · {kind}",
        group: Some(BoardGroup::NeedsYou),
    },
    EventMapping {
        kind: "commented",
        class: ChangeClass::Comment,
        template: "{task} · comment added",
        group: None,
    },
    EventMapping {
        kind: "annotation_*",
        class: ChangeClass::Annotation,
        template: "{task} · {kind}",
        group: Some(BoardGroup::Verifying),
    },
    EventMapping {
        kind: "automation_blocked",
        class: ChangeClass::Automation,
        template: "{task} · automation paused",
        group: Some(BoardGroup::Held),
    },
    EventMapping {
        kind: "automation_resumed",
        class: ChangeClass::Automation,
        template: "{task} · automation resumed",
        group: Some(BoardGroup::Held),
    },
    EventMapping {
        kind: "operating_policy_confirmed",
        class: ChangeClass::Policy,
        template: "{task} · operating policy confirmed",
        group: None,
    },
    EventMapping {
        kind: "queued",
        class: ChangeClass::Queue,
        template: "{task} · queued",
        group: Some(BoardGroup::Queued),
    },
    EventMapping {
        kind: "dispatch_failed",
        class: ChangeClass::Queue,
        template: "{task} · dispatch failed",
        group: Some(BoardGroup::Queued),
    },
    EventMapping {
        kind: "landing_stale",
        class: ChangeClass::Land,
        template: "{task} · landing candidate stale",
        group: Some(BoardGroup::Verifying),
    },
    EventMapping {
        kind: "autopilot_error",
        class: ChangeClass::Automation,
        template: "{task} · automation error",
        group: Some(BoardGroup::Verifying),
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeLine {
    pub seq: u64,
    pub task: Option<String>,
    pub at: String,
    pub kind: String,
    pub class: ChangeClass,
    pub text: String,
    pub group: Option<BoardGroup>,
}

/// Map one already-loaded event. No store read or event filtering occurs here.
pub fn change_line(event: &StoredEvent) -> ChangeLine {
    let mapping = (!event.malformed)
        .then(|| {
            EVENT_MAPPINGS.iter().find(|mapping| {
                mapping.kind.strip_suffix('*').map_or_else(
                    || mapping.kind == event.event.kind,
                    |prefix| event.event.kind.starts_with(prefix),
                )
            })
        })
        .flatten();
    let template = mapping.map_or("{task} · {kind}", |mapping| mapping.template);
    let detail = &event.event.detail;
    let result = detail
        .get("result")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("updated");
    let verdict = detail
        .get("verdict")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("updated");
    // Substitute tokens from the template once: event text cannot inject another token.
    let mut text = String::new();
    for (i, token) in template.split('{').enumerate() {
        if i == 0 {
            text.push_str(token);
            continue;
        }
        if let Some((name, rest)) = token.split_once('}') {
            text.push_str(match name {
                "task" => event.event.task.as_deref().unwrap_or("workspace"),
                "kind" => &event.event.kind,
                "result" => result,
                "verdict" => verdict,
                _ => name,
            });
            text.push_str(rest);
        }
    }
    let text = text
        .split_whitespace()
        .fold(String::with_capacity(text.len()), |mut line, word| {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            line
        });
    ChangeLine {
        seq: event.seq.0,
        task: event.event.task.clone(),
        at: event.event.ts.to_string(),
        kind: event.event.kind.clone(),
        class: mapping.map_or(ChangeClass::Other, |mapping| mapping.class),
        text,
        group: mapping.and_then(|mapping| mapping.group),
    }
}

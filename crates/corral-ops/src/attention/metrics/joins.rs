//! Action attribution: each action joins a condition only through an exact identity.
use std::collections::{BTreeMap, BTreeSet};

use corral_core::Event;

use super::{Actions, ConditionRow, Fact, Window, open_at, resolved_after, time, when, within};

/// The join identity an action event offers, and the condition-kind family it may attach to.
/// A missing identity field yields an identity that matches nothing.
fn join(event: &Event) -> Option<(&'static [&'static str], String)> {
    const DELIVERY: &[&str] = &["delivery_"];
    const RUN: &[&str] = &[
        "delivery_",
        "infra_escalation",
        "escalation",
        "agent_lost",
        "silent_turn",
        "stuck_process",
        "unexpected_stall",
    ];
    let detail = &event.detail;
    let field = |name: &str| detail[name].as_str().unwrap_or_default();
    // An inbox action's message ID names its recipient container, hence its run.
    let run = || match detail["run"].as_str() {
        Some(run) => run.to_owned(),
        None => field("message")
            .split('#')
            .next()
            .and_then(|key| key.strip_prefix(&format!("{}_", event.task.as_deref()?)))
            .unwrap_or_default()
            .to_owned(),
    };
    Some(match event.kind.as_str() {
        "message_retried" => (DELIVERY, format!("message:{}", field("from"))),
        "question_answered" => (
            &["question_"],
            format!("question:{}@{}", field("key"), detail["revision"]),
        ),
        "unparked" => (&["park_"], format!("park:{}", detail["revision"])),
        // A new park revision replaces, and so responds to, the previous revision's
        // conditions. The initial park creates the park and responds to nothing.
        "parked" => (
            &["park_"],
            format!(
                "park:{}",
                (detail["park"]["revision"].as_u64())
                    .and_then(|revision| revision.checked_sub(1))
                    .filter(|previous| *previous > 0)
                    .map_or_else(String::new, |previous| previous.to_string())
            ),
        ),
        "message_posted" => (DELIVERY, format!("recipient:{}", field("key"))),
        "replied" | "changes_requested" => (RUN, format!("run:{}", run())),
        "dispatched" | "resumed" | "automation_resumed" | "gate_started" | "review_started" => {
            (&RUN[1..], format!("run:{}", run()))
        }
        "moved" => (&["unexpected_stall"], "status:".into()),
        _ => return None,
    })
}

fn candidate(key: &str, row: &ConditionRow, family: &[&str], identity: &str) -> bool {
    if !family.iter().any(|prefix| row.kind.starts_with(prefix)) {
        return false;
    }
    match identity.split_once(':') {
        Some(("message" | "question" | "park", id)) => {
            !id.is_empty() && key.ends_with(&format!("/{id}"))
        }
        Some(("recipient", recipient)) => {
            !recipient.is_empty() && key.contains(&format!("/{recipient}#"))
        }
        Some(("run", run)) if row.kind.starts_with("delivery_") => {
            !run.is_empty() && key.contains(&format!("/{}_{run}#", row.task))
        }
        Some(("run", run)) => !run.is_empty() && row.run.as_deref() == Some(run),
        Some(("status", _)) => true,
        _ => false,
    }
}

/// The inbox message an action produced, whose observation or terminal state settles it.
fn effect(event: &Event) -> Option<String> {
    let field = match event.kind.as_str() {
        "message_retried" => "to",
        "message_posted" => "id",
        "replied" | "changes_requested" => "message",
        _ => return None,
    };
    event.detail[field].as_str().map(Into::into)
}

pub(super) fn actions(
    events: &[Event],
    facts: &[Fact],
    rows: &BTreeMap<String, ConditionRow>,
    window: Window,
) -> Actions {
    let mut out = Actions::default();
    let mut settled = BTreeMap::<String, (usize, bool)>::new();
    for (pos, event) in events.iter().enumerate() {
        let (detail, kind) = (&event.detail, event.kind.as_str());
        let terminal = kind.starts_with("message_")
            && !matches!(
                &kind[8..],
                "posted" | "admitted" | "retried" | "gap" | "terminal_noop"
            );
        let id = match kind {
            "message_observed" => detail["id"].as_str().map(String::from),
            _ if terminal => (detail["key"].as_str().zip(detail["seq"].as_u64()))
                .map(|(key, seq)| format!("{key}#{seq}")),
            _ => None,
        };
        if let Some(id) = id {
            settled
                .entry(id)
                .or_insert((pos, kind != "message_observed"));
        }
    }
    // A message's own post orders and times it: appended in the write that creates it, it is
    // causal log order, not publication order or a raw clock comparison.
    let posts: BTreeMap<_, _> = (events.iter().enumerate())
        .filter(|(_, event)| event.kind == "message_posted")
        .filter_map(|(pos, event)| Some((event.detail["id"].as_str()?, (pos, when(event)))))
        .collect();
    // One durable effect can be logged twice: its post and the retry, reply or change request
    // naming it. The naming event carries the identity; the post keeps the effect's position
    // and time, so a later naming event never moves the action across a report boundary.
    let named: BTreeSet<_> = (events.iter())
        .filter(|event| event.kind != "message_posted")
        .filter_map(effect)
        .filter(|id| posts.contains_key(id.as_str()))
        .collect();
    let mut prior = BTreeMap::<(String, String), Option<String>>::new();
    // Normalize first: each action takes its effect's post position and time. Then consume the
    // actions in that causal order, so a delayed naming event cannot reorder prior effects.
    let mut normalized: Vec<_> = (events.iter().enumerate())
        .filter_map(|(pos, event)| {
            let message = effect(event);
            let named_post = event.kind == "message_posted"
                && message.as_ref().is_some_and(|id| named.contains(id));
            let (family, identity) = join(event).filter(|_| !named_post)?;
            let (pos, at) = (message.as_deref())
                .and_then(|id| posts.get(id).copied())
                .unwrap_or((pos, when(event)));
            Some((pos, at, event, message, family, identity))
        })
        .collect();
    normalized.sort_by_key(|(pos, ..)| *pos);
    // History before the window seeds prior effects; only in-window actions are charged.
    for (pos, at, event, message, family, identity) in normalized {
        let charged = within(Some(at), window);
        let own = message.as_ref().map(|id| format!("/{id}"));
        let matches: Vec<_> = (rows.iter())
            .filter(|(key, row)| {
                // A delivery condition is about one message: it opens with that message's post.
                let source = (row.kind.starts_with("delivery_"))
                    .then(|| key.rsplit('/').next().and_then(|id| posts.get(id)))
                    .flatten();
                let open = match source {
                    Some((source, _)) => *source < pos && resolved_after(row, at),
                    None => open_at(row, at),
                };
                Some(row.task.as_str()) == event.task.as_deref()
                    && open
                    && own.as_ref().is_none_or(|own| !key.ends_with(own.as_str()))
                    && candidate(key, row, family, &identity)
            })
            .collect();
        let [(key, row)] = matches.as_slice() else {
            if charged && matches.is_empty() {
                out.unattributed += 1;
            } else if charged {
                out.ambiguous += 1;
            }
            continue;
        };
        // Duplicate state follows the delivery identity: a post, reply or change request is a
        // fresh post to its recipient; a retry keeps its exact source message.
        let effect_identity = match (event.kind.as_str(), &message) {
            ("message_retried", _) | (_, None) => identity,
            (_, Some(id)) => format!("recipient:{}", id.split('#').next().unwrap_or_default()),
        };
        // A repeat is a duplicate only while the earlier effect is still outstanding:
        // neither observed nor terminal before this action.
        let repeat = match prior.insert(((*key).clone(), effect_identity), message) {
            Some(Some(previous)) => match settled.get(&previous) {
                Some((settled, terminal)) if *settled < pos => Some(*terminal),
                _ => None,
            },
            Some(None) | None => Some(false),
        };
        if !charged {
            continue;
        }
        out.attributed += 1;
        *out.by_condition.entry((*key).clone()).or_default() += 1;
        let owner =
            matches!(event.actor, Some(corral_core::Actor::Owner)) || event.detail["by"] == "owner";
        out.owner_rescues += u64::from(owner && row.responsible.as_deref() == Some("pm"));
        match repeat {
            None => out.duplicates += 1,
            Some(true) => out.after_terminal += 1,
            Some(false) => {}
        }
    }
    for fact in facts.iter().filter(|fact| fact.kind == "attention_handled") {
        let owner = fact.detail["handling"]["by"] == "owner";
        let pm = fact.detail["key"]
            .as_str()
            .and_then(|key| rows.get(key))
            .is_some_and(|row| row.responsible.as_deref() == Some("pm"));
        out.owner_rescues +=
            u64::from(owner && pm && within(time(&fact.detail["handling"]["at"]), window));
    }
    out
}

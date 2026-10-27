//! Action attribution: each action joins a condition only through an exact identity.
use std::collections::{BTreeMap, BTreeSet};

use corral_core::Event;

use super::{Actions, ConditionRow, Fact, Window, open_at, time, when, within};

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
        "parked" => (&["park_"], format!("park:{}", detail["park"]["revision"])),
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
    let mut prior = BTreeMap::<(String, String), Option<String>>::new();
    // One message effect can be logged twice: as its post and as the retry, reply or change
    // request that names it. The naming event is the action; its post is not another one.
    let named: BTreeSet<_> = (events.iter())
        .filter_map(|event| match event.kind.as_str() {
            "message_retried" => event.detail["to"].as_str(),
            "replied" | "changes_requested" => event.detail["message"].as_str(),
            _ => None,
        })
        .collect();
    // History before the window seeds prior effects; only in-window actions are charged.
    for (pos, event) in events.iter().enumerate() {
        let named_post = event.kind == "message_posted"
            && (event.detail["id"].as_str()).is_some_and(|id| named.contains(id));
        let Some((family, identity)) = join(event).filter(|_| !named_post) else {
            continue;
        };
        let at = when(event);
        let charged = within(Some(at), window);
        // An action whose own effect is a condition's message created that condition; it
        // cannot also be a response to it.
        let own = effect(event).map(|id| format!("/{id}"));
        let matches: Vec<_> = (rows.iter())
            .filter(|(key, row)| {
                Some(row.task.as_str()) == event.task.as_deref()
                    && open_at(row, at)
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
        // A repeat through the same identity is a duplicate only while the earlier effect is
        // still outstanding: neither observed nor terminal before this action.
        let repeat = match prior.insert(((*key).clone(), identity), effect(event)) {
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

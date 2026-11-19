//! The selected agent's detail on the Agents tab: its brief (clipped, with "more"), what it owns
//! and waits for, what it does now, its edits, the owner's steers, its question, its report
//! (clipped, with "more"), its error and verdict, read from the host's `SessionChild` part with
//! the fleet row's facts until that is read; then its controls. Also the model choices of an
//! agent whose model the owner changes: the models the fleet approved, then the profiles' models
//! it did not, each unapproved one listed with why it cannot be chosen.

use std::collections::BTreeSet;

use corral_protocol::detail::SessionChildDetail;
use corral_protocol::subscription::{SessionChild, SessionChildVerdict};

use crate::model::Id;
use crate::model::agents::clock;
use crate::model::board::Mark;
use crate::model::hue::Hue;

use super::super::read_state::ReadState;
use super::super::status::cost_text;
use super::super::{Action, Cx, DockAction};
use super::{
    CHILD_CONTROL, FleetAction, FleetButton, asks, card_of, child_detail, child_target, control,
    ctx, helper, live, ops, profile_model, shown_state, time, turns,
};

/// A long text clipped to its first lines until the owner opens it.
const CLIP_LINES: usize = 6;
const CLIP_BYTES: usize = 700;
/// Edits listed before "more".
const EDITS_SHOWN: usize = 8;

/// The selected agent's facts and controls.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDetail {
    pub child: String,
    pub title: String,
    pub mark: Mark,
    pub hue: Hue,
    pub state: &'static str,
    /// `slice · gpt-6 · high effort · 1m 02s · $0.21 · ctx 18% · 4 turns`.
    pub facts: String,
    pub brief: Option<Clipped>,
    /// `Owns: src/auth/token.rs · starts after a0`.
    pub owns: Option<String>,
    /// What it does now, while it works.
    pub activity: Option<String>,
    pub edits: Option<EditList>,
    /// `14:02 · use the existing cache`, oldest first.
    pub steers: Vec<String>,
    /// `▲ Asks: …`, pressing it docks the question's card.
    pub asks: Option<(String, Option<Action>)>,
    pub report: Option<Clipped>,
    pub verdict: Option<(Hue, String)>,
    /// A review verdict's findings, `high · src/a.rs:12 · …`.
    pub findings: Vec<String>,
    pub pending_model: Option<String>,
    pub outside: Option<String>,
    pub error: Option<String>,
    /// `! 12 earlier messages were dropped by the retention limit`.
    pub dropped: Option<String>,
    /// Where the read of its brief, edits and report stands while it is not simply read.
    pub read: Option<(Hue, String)>,
    pub retry: Option<Action>,
    pub buttons: Vec<FleetButton>,
}

/// A long text, clipped until opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clipped {
    pub text: String,
    /// `more` or `less`, with its test id and its action; none when the text is short.
    pub more: Option<(String, &'static str, Action)>,
    /// Said when the host itself sent only the first part.
    pub note: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditList {
    /// `Edits · 3 files · 1 outside its own`.
    pub heading: String,
    pub lines: Vec<EditLine>,
    /// `12 more` or `fewer`.
    pub more: Option<(String, Action)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditLine {
    pub path: String,
    /// Outside the files it owns.
    pub outside: bool,
    pub at: Option<String>,
}

/// An agent's model choices.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPicker {
    pub child: String,
    /// `Change a1's model`.
    pub title: String,
    /// `Now: provider/model · switches to … at its next turn`.
    pub current: String,
    /// Why there is nothing to choose, or why nothing can be chosen now.
    pub note: Option<(Hue, String)>,
    pub choices: Vec<ModelChoiceRow>,
    pub close: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoiceRow {
    /// `seg-child-model-<n>`.
    pub id: String,
    pub label: String,
    /// The profiles that use it and what they are for.
    pub detail: Option<String>,
    pub current: bool,
    pub unavailable: Option<String>,
    pub action: Action,
}

/// The first lines of `text`, and whether anything was cut.
fn clip(text: &str) -> (String, bool) {
    let mut end = text.len();
    if let Some((at, _)) = text.match_indices('\n').nth(CLIP_LINES - 1) {
        end = at;
    }
    if end > CLIP_BYTES {
        end = CLIP_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    if end >= text.len() {
        return (text.to_owned(), false);
    }
    (format!("{}…", text[..end].trim_end()), true)
}

fn clipped(
    cx: &Cx<'_>,
    key: String,
    test_id: &'static str,
    text: &str,
    note: Option<&'static str>,
) -> Clipped {
    let open = cx.ui.fleet.opened.contains(&key);
    let (short, cut) = clip(text);
    Clipped {
        text: if open { text.to_owned() } else { short },
        more: cut.then(|| {
            (
                if open { "less" } else { "more" }.to_owned(),
                test_id,
                Action::Fleet(FleetAction::More {
                    session: cx.id.clone(),
                    key,
                }),
            )
        }),
        note,
    }
}

/// The time an RFC 3339 stamp shows as, when it parses.
fn at(stamp: &str, cx: &Cx<'_>) -> Option<String> {
    clock(Some(stamp), cx.feed)
}

fn facts(child: &SessionChild, served: Option<&SessionChildDetail>, now: u64) -> String {
    let mut facts = vec![profile_model(child)];
    if let Some(effort) = served
        .and_then(|detail| detail.effort.clone())
        .or_else(|| child.effort.clone())
    {
        facts.push(format!("{effort} effort"));
    }
    facts.push(time(child, now));
    facts.push(cost_text(child.cost_usd, false));
    facts.push(format!("ctx {}", ctx(child)));
    facts.push(turns(child.turns));
    facts.join(" · ")
}

fn owns(child: &SessionChild, served: Option<&SessionChildDetail>) -> Option<String> {
    let mut words = Vec::new();
    match served {
        Some(detail) if detail.owns.is_empty() => words.push("Owns nothing (read-only)".to_owned()),
        Some(detail) => {
            let shown: Vec<&str> = detail.owns.iter().take(4).map(String::as_str).collect();
            let rest = detail.owns.len().saturating_sub(shown.len());
            words.push(if rest > 0 {
                format!("Owns: {} and {rest} more", shown.join(", "))
            } else {
                format!("Owns: {}", shown.join(", "))
            });
        }
        None if child.owns > 0 => words.push(format!(
            "Owns {} {}",
            child.owns,
            if child.owns == 1 { "file" } else { "files" }
        )),
        None => {}
    }
    let after = served.map_or(&child.after, |detail| &detail.after);
    if !after.is_empty() {
        words.push(format!("starts after {}", after.join(", ")));
    }
    (!words.is_empty()).then(|| words.join(" · "))
}

fn edits(cx: &Cx<'_>, child: &str, detail: &SessionChildDetail) -> Option<EditList> {
    if detail.edits.is_empty() {
        return None;
    }
    let key = format!("edits:{child}");
    let open = cx.ui.fleet.opened.contains(&key);
    let paths: BTreeSet<&str> = detail.edits.iter().map(|edit| edit.path.as_str()).collect();
    let outside: BTreeSet<&str> = detail
        .edits
        .iter()
        .filter(|edit| edit.outside)
        .map(|edit| edit.path.as_str())
        .collect();
    let mut heading = format!(
        "Edits · {} {}",
        paths.len(),
        if paths.len() == 1 { "file" } else { "files" }
    );
    if !outside.is_empty() {
        heading.push_str(&format!(" · {} outside its own", outside.len()));
    }
    // Newest first, as the terminal lists them.
    let lines: Vec<EditLine> = detail
        .edits
        .iter()
        .rev()
        .take(if open { usize::MAX } else { EDITS_SHOWN })
        .map(|edit| EditLine {
            path: edit.path.clone(),
            outside: edit.outside,
            at: at(&edit.at, cx),
        })
        .collect();
    let hidden = detail.edits.len().saturating_sub(EDITS_SHOWN);
    Some(EditList {
        heading,
        lines,
        more: (hidden > 0).then(|| {
            (
                if open {
                    "fewer".to_owned()
                } else {
                    format!("{hidden} more")
                },
                Action::Fleet(FleetAction::More {
                    session: cx.id.clone(),
                    key,
                }),
            )
        }),
    })
}

fn verdict(
    child: &SessionChild,
    served: Option<&SessionChildDetail>,
) -> (Option<(Hue, String)>, Vec<String>) {
    if let Some(verdict) = served.and_then(|detail| detail.verdict.as_ref()) {
        let (hue, word) = match verdict.verdict.as_str() {
            "approve" | "approved" => (Hue::Ok, "approved"),
            "changes" | "changes_requested" => (Hue::Held, "changes requested"),
            _ => (Hue::Neutral, verdict.verdict.as_str()),
        };
        let mut words = format!("Verdict: {word}");
        if !verdict.summary.trim().is_empty() {
            words.push_str(&format!(" · {}", verdict.summary.trim()));
        }
        let findings = verdict
            .findings
            .iter()
            .map(|finding| {
                let place = match (&finding.path, finding.line) {
                    (Some(path), Some(line)) => format!(" · {path}:{line}"),
                    (Some(path), None) => format!(" · {path}"),
                    _ => String::new(),
                };
                format!("{}{place} · {}", finding.severity, finding.text)
            })
            .collect();
        return (Some((hue, words)), findings);
    }
    let verdict = child.verdict.map(|verdict| match verdict {
        SessionChildVerdict::Approve => (Hue::Ok, "Verdict: approved".to_owned()),
        SessionChildVerdict::Changes => (Hue::Held, "Verdict: changes requested".to_owned()),
    });
    (verdict, Vec::new())
}

/// The child's controls; `opened` leaves out the way into the conversation already shown.
pub(super) fn buttons(cx: &Cx<'_>, child: &SessionChild, opened: bool) -> Vec<FleetButton> {
    let session = cx.id.clone();
    let id = child.id.clone();
    let served = child_detail(cx.feed, cx.id, &id);
    let mut out = Vec::new();
    if !opened {
        out.push(
            FleetButton::new(
                "child-open",
                "Open conversation",
                Some("o"),
                Action::Fleet(FleetAction::Open {
                    session: session.clone(),
                    child: id.clone(),
                }),
            )
            .unless(
                served
                    .filter(|detail| !detail.transcript)
                    .map(|_| "it has recorded no conversation yet".to_owned()),
            ),
        );
    }
    if helper(child) {
        return out;
    }
    let control = control(cx, CHILD_CONTROL);
    let sending = ops::sending(cx, &id).then(|| "your last request is on its way".to_owned());
    if asks(child) {
        let card = card_of(cx, &id);
        out.push(
            FleetButton::new(
                "child-answer",
                "Answer",
                Some("a"),
                card.clone().map_or(
                    Action::SelectAgent {
                        session: session.clone(),
                        child: Some(id.clone()),
                    },
                    |card| {
                        Action::Dock(DockAction::Bring {
                            session: session.clone(),
                            card,
                        })
                    },
                ),
            )
            .unless(control.clone())
            .unless(
                card.is_none()
                    .then(|| "its question has not reached this device yet".into()),
            ),
        );
    }
    let not_steerable = (!child.steerable).then(|| {
        if live(child) {
            "it takes a message only while it runs".to_owned()
        } else {
            "it has finished".to_owned()
        }
    });
    out.push(
        FleetButton::new(
            "child-steer",
            "Steer",
            Some("s"),
            Action::Fleet(FleetAction::Steer {
                session: session.clone(),
                child: id.clone(),
            }),
        )
        .unless(control.clone())
        .unless(not_steerable),
    );
    if live(child) {
        out.push(
            FleetButton::new(
                "child-model",
                "Change model",
                Some("m"),
                Action::Fleet(FleetAction::Models {
                    session: session.clone(),
                    child: Some(id.clone()),
                }),
            )
            .unless(control.clone())
            .unless(sending.clone()),
        );
        out.push(
            FleetButton::new(
                "child-cancel",
                "Cancel",
                Some("x"),
                Action::Fleet(FleetAction::Cancel { session, child: id }),
            )
            .danger()
            .unless(control)
            .unless(sending),
        );
    }
    out
}

/// The selected agent's detail.
pub(super) fn detail(cx: &Cx<'_>, child: &SessionChild) -> AgentDetail {
    let target = child_target(cx.id, &child.id);
    let cache = cx.feed.detail(&target);
    let served = child_detail(cx.feed, cx.id, &child.id);
    let read = ReadState::of(cache, cx.feed);
    let (mark, hue, state) = shown_state(child);
    let (verdict, findings) = verdict(child, served);
    let question = served
        .and_then(|detail| detail.question.as_ref())
        .map(|question| question.question.clone());
    let card = card_of(cx, &child.id);
    let unread = match &read {
        ReadState::Failed(_) | ReadState::Stale { .. } => Some((read.hue(), read.words())),
        ReadState::NotRead | ReadState::Reading if served.is_none() => {
            Some(match control(cx, super::CHILDREN) {
                Some(why) => (
                    Hue::Held,
                    format!("Its brief, edits and report are not read: {why}"),
                ),
                None => (
                    Hue::Neutral,
                    "Reading its brief, edits and report…".to_owned(),
                ),
            })
        }
        _ => None,
    };
    AgentDetail {
        child: child.id.clone(),
        title: served.map_or_else(|| child.title.clone(), |detail| detail.title.clone()),
        mark,
        hue,
        state,
        facts: facts(child, served, cx.feed.now),
        brief: served
            .filter(|detail| !detail.brief.trim().is_empty())
            .map(|detail| {
                clipped(
                    cx,
                    format!("brief:{}", child.id),
                    "btn-more-brief",
                    &detail.brief,
                    detail.brief_more.then_some(
                        "The host sent the brief's first 8 KiB; its conversation holds all of it.",
                    ),
                )
            }),
        owns: owns(child, served),
        activity: live(child)
            .then(|| {
                served
                    .and_then(|detail| detail.activity.clone())
                    .or_else(|| child.activity.clone())
            })
            .flatten(),
        edits: served.and_then(|detail| edits(cx, &child.id, detail)),
        steers: served.map_or_else(Vec::new, |detail| {
            detail
                .steers
                .iter()
                .map(
                    |steer| match steer.at.as_deref().and_then(|stamp| at(stamp, cx)) {
                        Some(at) => format!("{at} · {}", steer.text),
                        None => steer.text.clone(),
                    },
                )
                .collect()
        }),
        asks: asks(child).then(|| {
            let words = match &question {
                Some(question) => format!("▲ Asks: {question}"),
                None => "▲ Waits for your answer".to_owned(),
            };
            let bring = card.map(|card| {
                Action::Dock(DockAction::Bring {
                    session: cx.id.clone(),
                    card,
                })
            });
            (words, bring)
        }),
        report: served
            .and_then(|detail| detail.report.as_deref().map(|report| (detail, report)))
            .filter(|(_, report)| !report.trim().is_empty())
            .map(|(detail, report)| {
                clipped(
                    cx,
                    format!("report:{}", child.id),
                    "btn-more-report",
                    report,
                    detail
                        .report_more
                        .then_some("The host sent the report's first 16 KiB."),
                )
            }),
        verdict,
        findings,
        pending_model: served
            .and_then(|detail| detail.pending_model.clone())
            .or_else(|| child.pending_model.clone()),
        outside: (child.outside > 0).then(|| {
            format!(
                "{} {} outside its own files",
                child.outside,
                if child.outside == 1 { "edit" } else { "edits" }
            )
        }),
        error: served
            .and_then(|detail| detail.error.clone())
            .or_else(|| child.error.clone()),
        dropped: served
            .and_then(|detail| detail.dropped)
            .filter(|n| *n > 0)
            .map(|n| {
                format!(
                    "! {n} earlier {} dropped by the retention limit",
                    if n == 1 {
                        "message was"
                    } else {
                        "messages were"
                    }
                )
            }),
        retry: matches!(read, ReadState::Failed(_)).then_some(Action::Refresh(target)),
        read: unread,
        buttons: buttons(cx, child, false),
    }
}

/// The open model choices of the agent the owner re-models.
pub(super) fn picker(cx: &Cx<'_>) -> Option<ModelPicker> {
    let child = cx.ui.fleet.picking.clone()?;
    let row = super::fleet(cx)?
        .children
        .iter()
        .find(|row| row.id == child)?;
    let session: &Id = cx.id;
    let served = child_detail(cx.feed, session, &child);
    let current = served.map_or_else(|| row.model.clone(), |detail| detail.model.clone());
    let pending = served
        .and_then(|detail| detail.pending_model.clone())
        .or_else(|| row.pending_model.clone());
    let blocked = control(cx, CHILD_CONTROL)
        .or_else(|| ops::sending(cx, &child).then(|| "your last request is on its way".into()));
    let choose = |model: &str| {
        Action::Fleet(FleetAction::SetModel {
            session: session.clone(),
            child: child.clone(),
            model: model.to_owned(),
        })
    };
    let mut choices = Vec::new();
    if let Some(detail) = served {
        let profiles = |model: &str| {
            let named: Vec<String> = detail
                .profiles
                .iter()
                .filter(|profile| profile.model == model)
                .map(|profile| match &profile.description {
                    Some(about) => format!("{} · {about}", profile.name),
                    None => profile.name.clone(),
                })
                .collect();
            (!named.is_empty()).then(|| named.join("; "))
        };
        let mut seen = BTreeSet::new();
        let approved = detail.models.iter().map(|model| (model, None));
        let unapproved = detail
            .profiles
            .iter()
            .map(|profile| &profile.model)
            .filter(|model| !detail.models.contains(*model))
            .map(|model| {
                (
                    model,
                    Some("not approved for this session's agents".to_owned()),
                )
            });
        for (model, refused) in approved.chain(unapproved) {
            if !seen.insert(model.clone()) {
                continue;
            }
            let is_current = *model == current;
            let mut about = profiles(model.as_str());
            if pending.as_deref() == Some(model.as_str()) {
                about = Some(match about {
                    Some(about) => format!("switches at its next turn · {about}"),
                    None => "switches at its next turn".into(),
                });
            }
            choices.push(ModelChoiceRow {
                id: format!("seg-child-model-{}", choices.len()),
                label: model.clone(),
                detail: about,
                current: is_current,
                unavailable: refused
                    .or_else(|| is_current.then(|| "it runs on this model now".into()))
                    .or_else(|| blocked.clone()),
                action: choose(model.as_str()),
            });
        }
    }
    let read = ReadState::of(cx.feed.detail(&child_target(session, &child)), cx.feed);
    let note = match served {
        None if matches!(read, ReadState::Failed(_)) => Some((Hue::Fail, read.words())),
        None => Some((Hue::Neutral, "Reading the models it may use…".to_owned())),
        Some(_) if choices.is_empty() => Some((
            Hue::Neutral,
            "The host offers no model choices for this agent.".to_owned(),
        )),
        Some(_) => blocked.map(|why| (Hue::Held, format!("unavailable: {why}"))),
    };
    let mut now = format!("Now: {current}");
    if let Some(pending) = pending.filter(|pending| *pending != current) {
        now.push_str(&format!(" · switches to {pending} at its next turn"));
    }
    Some(ModelPicker {
        title: format!("Change {child}'s model"),
        current: now,
        note,
        choices,
        close: Action::Fleet(FleetAction::Models {
            session: session.clone(),
            child: None,
        }),
        child,
    })
}

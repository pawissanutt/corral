//! The stamps a served handover carries: the canonical answer, this device's answer or
//! follow-up outcome (pending, refused, conflicting, unknown, abandoned), why the app cannot
//! answer, and a stale copy.

use corral_protocol::detail::QuestionDetail;
use corral_protocol::rows::{InboxRow, InboxState};

use super::live::{Draft, Sent, clock, question_ref, sent_status};
use super::sheet::button;
use super::{Action, ButtonKind, Notice, QuestionState, State};
use crate::model::Id;
use crate::model::act::ActStatus;
use crate::model::daemon::{Feed, Link};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

/// “option 2” for a recorded option label, else the written text quoted.
fn what(detail: &QuestionDetail, option: Option<&str>, text: &str) -> String {
    match option.and_then(|label| detail.options.iter().position(|o| o.label == label)) {
        Some(index) => format!("option {}", index + 1),
        None => {
            let short: String = text.chars().take(80).collect();
            let more = if text.chars().count() > 80 { "…" } else { "" };
            format!("“{short}{more}”")
        }
    }
}

fn sent_what(detail: &QuestionDetail, sent: &Sent) -> String {
    match sent.option {
        Some(index) => format!("option {}", index + 1),
        None => what(detail, None, &sent.text),
    }
}

fn notice(
    stamp: &'static str,
    hue: Hue,
    body: Rich,
    retry: Option<super::Button>,
) -> Option<Notice> {
    Some(Notice {
        stamp: Some(stamp),
        hue,
        body,
        retry,
    })
}

/// The stamp under the question: the canonical answer, this device's command outcome, why
/// the app cannot answer, or that the copy shown is stale.
pub(super) fn situation_notice(
    state: &State,
    feed: &Feed,
    detail: &QuestionDetail,
    row: Option<&InboxRow>,
    status: QuestionState,
) -> Option<Notice> {
    let key = Id::new(&detail.key);
    let sent = state
        .desk
        .draft(&detail.key)
        .and_then(|draft| draft.answer.as_ref());
    let row_answer = match row.map(|row| &row.state) {
        Some(InboxState::Answered { by, via, at }) => Some((by, via, at)),
        _ => None,
    };
    match status {
        QuestionState::Mine => {
            let body = match (&detail.answer, sent) {
                (Some(answer), _) => vec![
                    span(format!(
                        "By you at {} with {} · ",
                        clock(&answer.at, feed.now),
                        what(detail, answer.option.as_deref(), &answer.text)
                    )),
                    span(super::provenance(feed, &answer.by, answer.device.as_deref(), &answer.via)),
                    span(". Other devices and the PM pane now show it answered."),
                ],
                (None, Some(sent)) => vec![span(format!(
                    "corrald applied your answer with {} at rev {}. Other devices and the PM pane now show it answered.",
                    sent_what(detail, sent),
                    sent.rev
                ))],
                (None, None) => vec![span("Answered from this device.")],
            };
            notice("ANSWERED", Hue::Ok, body, None)
        }
        QuestionState::Elsewhere => {
            let body = match (&detail.answer, row_answer) {
                (Some(answer), _) => vec![
                    span(format!("Answered by {} at {} with {} · ", answer.by, clock(&answer.at, feed.now), what(detail, answer.option.as_deref(), &answer.text))),
                    span(super::provenance(feed, &answer.by, answer.device.as_deref(), &answer.via)),
                    span(". This copy is read-only; one answer wins across devices and the PM pane."),
                ],
                (None, Some((by, via, at))) => vec![
                    span(format!("Answered by {by} at {} · ", clock(at, feed.now))),
                    span(super::provenance(feed, by, None, via)),
                    span(". This copy is read-only; one answer wins across devices and the PM pane."),
                ],
                (None, None) => vec![span("Answered elsewhere. This copy is read-only.")],
            };
            notice("ANSWERED ELSEWHERE", Hue::Ok, body, None)
        }
        QuestionState::Closed => notice(
            if matches!(detail.state, InboxState::Expired) { "EXPIRED" } else { "CANCELLED" },
            Hue::Neutral,
            vec![span(format!(
                "corrald closed this question at rev {}; it no longer takes answers.",
                detail.rev
            ))],
            None,
        ),
        QuestionState::Pending => sent.and_then(|sent| {
            notice(
                "SENDING",
                Hue::Held,
                vec![span(format!(
                    "{} went to corrald at rev {} from this device; waiting for it to apply.",
                    capital(&sent_what(detail, sent)),
                    sent.rev
                ))],
                None,
            )
        }),
        QuestionState::Uncertain => sent.and_then(|sent| {
            notice(
                "UNKNOWN",
                Hue::Fail,
                vec![span(format!(
                    "corrald may or may not have applied {}: the connection dropped before its reply. It is never resent on its own. ",
                    sent_what(detail, sent)
                ))],
                Some(button("Check result", Action::Recover(sent.cmd.clone()), ButtonKind::Quiet)),
            )
        }),
        QuestionState::Open | QuestionState::Queued => {
            let outcome = sent.and_then(|sent| {
                let told = sent_what(detail, sent);
                match sent_status(feed, Some(sent))? {
                    ActStatus::Refused(reason) => notice(
                        "REFUSED",
                        Hue::Fail,
                        vec![span(format!("corrald refused {told}: {reason}. Nothing changed."))],
                        None,
                    ),
                    ActStatus::Failed(reason) => notice(
                        "FAILED",
                        Hue::Fail,
                        vec![span(format!("{} failed: {reason}.", capital(&told)))],
                        None,
                    ),
                    ActStatus::Conflict(reason) => notice(
                        "CONFLICT",
                        Hue::Held,
                        vec![span(format!(
                            "The question changed after rev {}, so {told} was not applied ({reason}). Read it again before answering.",
                            sent.rev
                        ))],
                        Some(button("Reload", Action::Refetch(key.clone()), ButtonKind::Quiet)),
                    ),
                    ActStatus::Abandoned => notice(
                        "NOT SENT",
                        Hue::Held,
                        vec![span(format!(
                            "corrald never received {told}. Answer again to send it."
                        ))],
                        None,
                    ),
                    _ => None,
                }
            });
            outcome
                .or_else(|| unanswerable(row))
                .or_else(|| stale(feed, detail))
        }
    }
}

fn unanswerable(row: Option<&InboxRow>) -> Option<Notice> {
    let row = row.filter(|row| !row.answerable)?;
    let body = match row.unanswerable_reason.as_deref() {
        Some("answer_in_pane") => vec![span(
            "This question is answered in its agent's pane, not from the app.",
        )],
        Some(reason) => vec![
            span("corrald does not take this answer from the app: "),
            span(reason.to_owned()).mono(),
            span("."),
        ],
        None => vec![span("corrald does not take this answer from the app.")],
    };
    notice("ANSWER IN PANE", Hue::Neutral, body, None)
}

fn stale(feed: &Feed, detail: &QuestionDetail) -> Option<Notice> {
    let cache = feed.detail(&question_ref(&detail.key))?;
    (cache.stale || feed.link != Link::Up).then(|| Notice {
        stamp: Some("STALE"),
        hue: Hue::Held,
        body: vec![span(match &feed.synced {
            Some(at) => format!(
                "The last copy received at {at}; it may be out of date until corrald is back."
            ),
            None => "The last copy received; it may be out of date until corrald is back.".into(),
        })],
        retry: None,
    })
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The follow-up's pending, refused, conflicting or unknown state under the reply field.
pub(super) fn followup_status(
    feed: &Feed,
    detail: &QuestionDetail,
    draft: Option<&Draft>,
) -> Option<Notice> {
    let sent = draft.and_then(|draft| draft.followup.as_ref())?;
    let (hue, body, retry) = match sent_status(feed, Some(sent))? {
        ActStatus::Previewing
        | ActStatus::Confirming
        | ActStatus::Queued
        | ActStatus::Pending
        | ActStatus::Running => (
            Hue::Held,
            "Sending your follow-up to the asker…".to_owned(),
            None,
        ),
        ActStatus::Unknown => (
            Hue::Fail,
            "corrald may or may not have posted your follow-up; it is never resent on its own."
                .to_owned(),
            Some(button(
                "Check result",
                Action::Recover(sent.cmd.clone()),
                ButtonKind::Quiet,
            )),
        ),
        ActStatus::Refused(reason) => (
            Hue::Fail,
            format!("Refused: {reason}. Nothing was posted."),
            None,
        ),
        ActStatus::Failed(reason) => (Hue::Fail, format!("Not posted: {reason}."), None),
        ActStatus::Conflict(_) => (
            Hue::Held,
            format!(
                "The question changed after rev {}; the follow-up was not posted.",
                sent.rev
            ),
            Some(button(
                "Reload",
                Action::Refetch(Id::new(&detail.key)),
                ButtonKind::Quiet,
            )),
        ),
        ActStatus::Abandoned => (
            Hue::Held,
            "corrald never received the follow-up. Send it again.".to_owned(),
            None,
        ),
        ActStatus::Done => return None,
    };
    Some(Notice {
        stamp: None,
        hue,
        body: vec![span(body)],
        retry,
    })
}

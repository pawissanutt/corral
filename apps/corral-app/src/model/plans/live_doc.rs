//! A served revision under review: its document, owner decisions and decide bar; the plan
//! document outside review; the diff between two revisions; and the freshness gates acceptance
//! and requested changes pass before they leave as commands.

use corral_protocol::detail::{DetailBody, DetailRef, PlanDetail, PlanDocument};
use corral_protocol::rows::{PlanRow as WirePlan, PlanState as WireState};

use super::live::{plan_cache, project_of, stale_line, state_chip, title};
use super::{
    Action, AwaitingView, Button, ButtonKind, Chip, Compose, Decide, DecideNote, DecisionRow,
    DiffLine, DiffLineKind, DiffView, DocumentSection, DocumentView, Draft, Header, Lens,
    LiveAction, LiveState, ProposedRow, ReviewRow, SectionBody, Sent, State,
};
use crate::model::act::{self, ActStatus, Authority};
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span};
use crate::model::work::when;
use crate::model::{Id, Live};

const OFFLINE: &str = "Refused offline: acceptance is revision-checked when it applies.";

fn refused(text: impl Into<String>) -> DecideNote {
    DecideNote {
        text: text.into(),
        hue: Hue::Fail,
    }
}

fn waiting(text: impl Into<String>) -> DecideNote {
    DecideNote {
        text: text.into(),
        hue: Hue::Held,
    }
}

/// A sent command's status as the decide note says it.
fn status_note(what: &str, status: &ActStatus) -> DecideNote {
    DecideNote {
        text: format!("{what} · {}", status.line()),
        hue: match status {
            ActStatus::Done => Hue::Ok,
            ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => Hue::Fail,
            ActStatus::Unknown | ActStatus::Abandoned => Hue::Held,
            _ => Hue::Neutral,
        },
    }
}

/// The command sent about `plan` and where it stands, once the client journal knows it.
fn sent_status<'a>(
    sent: Option<&'a Sent>,
    feed: &Feed,
    plan: &str,
) -> Option<(&'a Sent, ActStatus)> {
    let sent = sent.filter(|sent| sent.plan == plan)?;
    act::status(feed, &sent.key).map(|status| (sent, status))
}

fn in_flight(sent: Option<&Sent>, feed: &Feed, plan: &str) -> bool {
    sent_status(sent, feed, plan).is_some_and(|(_, status)| !status.settled())
}

/// Requested changes for `plan` are sent and not settled.
pub(super) fn changes_in_flight(live: &LiveState, feed: &Feed, plan: &str) -> bool {
    in_flight(live.changes.as_ref(), feed, plan)
}

/// The note still open for `plan`: none once it was sent on the revision it names and corrald
/// has done it, so the field closes after success and a later revision never reopens it.
pub(super) fn open_draft<'a>(live: &'a LiveState, feed: &Feed, plan: &str) -> Option<&'a Draft> {
    let draft = live.draft_of(plan)?;
    let done = sent_status(live.changes.as_ref(), feed, plan).is_some_and(|(sent, status)| {
        status == ActStatus::Done && draft.revision.as_deref() == Some(sent.revision.as_str())
    });
    (!done).then_some(draft)
}

/// An acceptance for `plan` is sent and not settled.
pub(super) fn accept_in_flight(live: &LiveState, feed: &Feed, plan: &str) -> bool {
    in_flight(live.accept.as_ref(), feed, plan)
}

/// What every decision needs: a connection, `act` authority, and a current copy of a revision
/// that still awaits acceptance and matches the plan row.
fn fresh<'a>(feed: &'a Feed, id: &str, env: Env) -> Result<(&'a PlanDetail, String), DecideNote> {
    if env.conn.refuses() {
        return Err(refused(OFFLINE));
    }
    if !feed
        .session
        .as_ref()
        .is_some_and(|session| session.authority.act)
    {
        return Err(refused(Authority::Act.refusal()));
    }
    let cache = plan_cache(feed, id);
    let Some((cache, DetailBody::Plan(detail))) =
        cache.and_then(|cache| cache.body().map(|body| (cache, body)))
    else {
        return Err(waiting("Waiting for corrald's copy of this plan."));
    };
    if cache.loading || cache.stale || cache.error.is_some() {
        return Err(waiting(
            "Showing the last copy received; deciding waits for a current one.",
        ));
    }
    if detail.state != WireState::Awaiting {
        return Err(waiting("This plan no longer awaits acceptance."));
    }
    let Some(rev) = detail.revision.clone() else {
        return Err(waiting("corrald names no revision to decide on."));
    };
    if let Some(listed) = feed.plans.get(id).and_then(|row| row.revision.as_deref())
        && listed != rev
    {
        return Err(waiting(format!(
            "Revision {listed} arrived; waiting for its document."
        )));
    }
    Ok((detail, rev))
}

/// One `Resolved: <decision> => <resolution>` line per owner decision, the form the acceptance
/// guard matches; absent for a plan without owner decisions.
pub(super) fn notes(live: &LiveState, plan: &str, detail: &PlanDetail) -> Option<String> {
    if detail.owner_decisions.is_empty() {
        return None;
    }
    Some(
        detail
            .owner_decisions
            .iter()
            .map(|decision| {
                let answer = live.resolution_of(plan, decision).split_whitespace();
                format!(
                    "Resolved: {} => {}",
                    decision.trim(),
                    answer.collect::<Vec<_>>().join(" ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The revision acceptance would bind to, or why Accept is refused right now.
pub(super) fn accept_gate(
    live: &LiveState,
    feed: &Feed,
    id: &str,
    env: Env,
) -> Result<String, DecideNote> {
    let (detail, rev) = fresh(feed, id, env)?;
    if let Some((sent, status)) = sent_status(live.accept.as_ref(), feed, id) {
        if !status.settled() {
            return Err(status_note("Accept", &status));
        }
        if status == ActStatus::Done && sent.revision == rev {
            return Err(DecideNote {
                text: format!("Accepted {rev}"),
                hue: Hue::Ok,
            });
        }
    }
    if changes_in_flight(live, feed, id) {
        return Err(waiting("Requested changes are in flight."));
    }
    if !detail.acceptable {
        return Err(waiting(if detail.accept_blockers.is_empty() {
            "corrald does not accept this revision yet.".to_owned()
        } else {
            detail.accept_blockers.join("; ")
        }));
    }
    let open = detail
        .owner_decisions
        .iter()
        .filter(|decision| live.resolution_of(id, decision).trim().is_empty())
        .count();
    if open > 0 {
        return Err(waiting(match open {
            1 => "Resolve the owner decision before accepting.".to_owned(),
            n => format!("Resolve the {n} owner decisions before accepting."),
        }));
    }
    Ok(rev)
}

/// The revision a request for changes would answer, or why it is refused right now.
pub(super) fn changes_gate(
    live: &LiveState,
    feed: &Feed,
    id: &str,
    env: Env,
) -> Result<String, DecideNote> {
    let (_, rev) = fresh(feed, id, env)?;
    if let Some((_, status)) = sent_status(live.changes.as_ref(), feed, id)
        && !status.settled()
    {
        return Err(status_note("Request changes", &status));
    }
    if in_flight(live.accept.as_ref(), feed, id) {
        return Err(waiting("Acceptance is in flight."));
    }
    Ok(rev)
}

/// The current revision and the one submitted before it, from corrald's revision list.
pub(super) fn diff_range(detail: &PlanDetail) -> Option<(String, String)> {
    let to = detail.revision.clone()?;
    let from = earlier(detail).last()?.clone();
    Some((from, to))
}

/// Revisions submitted before the current one, in stored order.
fn earlier(detail: &PlanDetail) -> Vec<String> {
    let Some(to) = detail.revision.as_deref() else {
        return vec![];
    };
    let before = detail
        .revisions
        .iter()
        .position(|rev| rev == to)
        .unwrap_or(detail.revisions.len());
    detail.revisions[..before]
        .iter()
        .filter(|rev| rev.as_str() != to)
        .cloned()
        .collect()
}

fn bullets(heading: &'static str, items: &[String]) -> Option<DocumentSection> {
    (!items.is_empty()).then(|| DocumentSection {
        heading,
        body: SectionBody::Bullets(items.to_vec()),
    })
}

fn paragraph(heading: &'static str, text: Option<&String>) -> Option<DocumentSection> {
    text.map(|text| DocumentSection {
        heading,
        body: SectionBody::Paragraph(vec![span(text.as_str())]),
    })
}

fn review_mark(verdict: &str) -> Mark {
    match verdict {
        v if v.starts_with("approve") => Mark::Done,
        "changes" | "request_changes" | "request-changes" | "reject" | "rejected" => Mark::Fail,
        _ => Mark::Na,
    }
}

/// The document's sections in the mockup's order, with the reviews bound to their revision.
fn sections(doc: Option<&PlanDocument>, detail: &PlanDetail, feed: &Feed) -> Vec<DocumentSection> {
    let mut out = vec![];
    if let Some(doc) = doc {
        out.extend(paragraph("Problem", doc.problem.as_ref()));
        out.extend(paragraph("Goal", doc.goal.as_ref()));
        out.extend(bullets("Changes", &doc.changes));
        out.extend(bullets("Keeps", &doc.retained));
        out.extend(bullets("Not doing", &doc.non_goals));
        if !doc.proposed.is_empty() {
            out.push(DocumentSection {
                heading: "Proposed members · published on accept",
                body: SectionBody::Proposed(
                    doc.proposed
                        .iter()
                        .map(|member| ProposedRow {
                            mark: Mark::Next,
                            name: member.name.clone(),
                            sub: if member.needs.is_empty() {
                                "can start now".into()
                            } else {
                                format!("after {}", member.needs.join(", "))
                            },
                        })
                        .collect(),
                ),
            });
        }
    }
    if !detail.reviews.is_empty() {
        let current = detail.revision.as_deref();
        out.push(DocumentSection {
            heading: "Reviewer verdicts · bound to plan revision",
            body: SectionBody::Reviews(
                detail
                    .reviews
                    .iter()
                    .map(|review| {
                        let superseded = current.is_some_and(|rev| rev != review.revision);
                        let original = review_mark(&review.verdict);
                        let mut verdict = vec![span(review.verdict.as_str())];
                        if superseded {
                            verdict.push(span(" · superseded").dim());
                        }
                        let who = [
                            review.profile.clone(),
                            review
                                .at
                                .as_deref()
                                .map(|at| when(at, feed))
                                .filter(|at| !at.is_empty()),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                        let mut note = vec![];
                        if !who.is_empty() {
                            note.push(span(format!("{who} ·")).dim());
                        }
                        if let Some(text) = &review.note {
                            note.push(span(format!(" {text}")));
                        }
                        ReviewRow {
                            rev: review.revision.clone(),
                            original_mark: original,
                            mark: if superseded { Mark::Stale } else { original },
                            verdict,
                            note,
                            superseded,
                        }
                    })
                    .collect(),
            ),
        });
    }
    out
}

/// Where the diff button leads from here: the diff, or back from it.
fn diff_toggle(state: &State, id: &Id, detail: &PlanDetail) -> Option<Button> {
    if let Some(Lens::Diff { .. }) = state.live.lens_of(id) {
        return Some(
            Button::new(
                "Open document",
                Action::Live(if detail.state == WireState::Awaiting {
                    LiveAction::Overview(id.clone())
                } else {
                    LiveAction::Document(id.clone())
                }),
            )
            .quiet()
            .hinted("Back to the plan's document."),
        );
    }
    diff_range(detail).map(|(from, to)| {
        Button::new(
            format!("Diff {from} → {to}"),
            Action::Live(LiveAction::Diff(id.clone())),
        )
        .quiet()
        .hinted(format!("What changed between revisions {from} and {to}."))
    })
}

fn via(feed: &Feed) -> Option<String> {
    feed.session
        .as_ref()
        .map(|session| format!("via=app:{}", session.device.name))
}

/// The decide bar of an awaiting revision, with every refusal and command status resolved.
pub(super) fn decide(state: &State, feed: &Feed, id: &Id, detail: &PlanDetail, env: Env) -> Decide {
    let live = &state.live;
    let phone = env.form == Form::Phone;
    let off = env.conn.refuses();
    let members = detail.document.as_ref().map_or(0, |doc| doc.proposed.len());
    let gate = accept_gate(live, feed, id, env);
    let mut accept = Button::new("Accept plan", Action::Live(LiveAction::Accept(id.clone())))
        .hinted(match &gate {
            Ok(rev) if members > 0 => format!(
                "Accept {rev} after a fresh confirmation on this device; it publishes {members} member tasks."
            ),
            Ok(rev) => format!("Accept {rev} after a fresh confirmation on this device."),
            Err(refusal) => refusal.text.clone(),
        });
    accept.enabled = gate.is_ok();
    accept.primary = true;
    accept.kind = ButtonKind::Standard;
    let in_flight = changes_in_flight(live, feed, id);
    let compose = open_draft(live, feed, id).map(|draft| {
        // Enablement leaves the typed text out, so typing never rebuilds the bar under the
        // field; an empty note is not sent. A note about an earlier revision names the
        // revision Send would use, so nothing is retargeted unseen.
        let label = match (draft.revision.as_deref(), detail.revision.as_deref()) {
            (Some(wrote), Some(now)) if wrote != now => format!("Send on {now}"),
            _ => "Send".to_owned(),
        };
        let mut send = Button::new(label, Action::Live(LiveAction::SendChanges(id.clone())));
        send.enabled = changes_gate(live, feed, id, env).is_ok() && !in_flight;
        send.kind = ButtonKind::Standard;
        let mut cancel = Button::new(
            "Cancel",
            Action::Live(LiveAction::CancelChanges(id.clone())),
        )
        .quiet();
        cancel.enabled = !in_flight;
        Compose {
            plan: id.clone(),
            text: draft.text.clone(),
            placeholder: draft.revision.as_ref().map_or_else(
                || "What should change?".to_owned(),
                |rev| format!("What should change in {rev}?"),
            ),
            locked: in_flight,
            send,
            cancel,
        }
    });
    let mut buttons = vec![];
    if compose.is_none() {
        let changes_gate = changes_gate(live, feed, id, env);
        let mut changes = Button::new(
            "Request changes",
            Action::Live(LiveAction::Changes(id.clone())),
        )
        .hinted(match &changes_gate {
            Ok(rev) => format!("Write a request for changes to {rev}."),
            Err(refusal) => refusal.text.clone(),
        });
        changes.enabled = changes_gate.is_ok();
        buttons.push(accept);
        buttons.push(changes);
        // The phone's one Ask is its floating button, and its diff is in the page head.
        if !phone {
            buttons.push(
                Button::new(
                    "Ask",
                    Action::Live(LiveAction::Ask {
                        record: Live::Plan(id.clone()),
                        rev: feed.plans.get(id.as_str()).map(|row| row.rev),
                        to: None,
                        text: None,
                    }),
                )
                .quiet()
                .hinted("Open the shared chat with this plan attached."),
            );
            if !off {
                buttons.extend(diff_toggle(state, id, detail));
            }
        }
    }
    let changes = sent_status(live.changes.as_ref(), feed, id);
    let accepted = sent_status(live.accept.as_ref(), feed, id);
    for (sent, status) in [&changes, &accepted].into_iter().flatten() {
        if *status == ActStatus::Unknown {
            buttons.push(
                Button::new(
                    "Check result",
                    Action::Live(LiveAction::Recover(sent.key.clone())),
                )
                .quiet()
                .hinted(
                    "Ask corrald for the result of the uncertain command, under the same identity.",
                ),
            );
        }
    }
    // A settled command about an earlier revision says nothing about the one shown; unsettled
    // ones stay visible whatever revision they named.
    let current = |sent: &Sent, status: &ActStatus| {
        !status.settled() || detail.revision.as_deref() == Some(sent.revision.as_str())
    };
    let changes = changes.filter(|(sent, status)| current(sent, status));
    let accepted = accepted.filter(|(sent, status)| current(sent, status));
    // A note written about an earlier revision says so before anything else, so sending it on
    // the newer one is a choice the owner sees.
    let moved = open_draft(live, feed, id)
        .filter(|_| !in_flight)
        .and_then(
            |draft| match (draft.revision.as_deref(), detail.revision.as_deref()) {
                (Some(wrote), Some(now)) if wrote != now => {
                    Some((wrote.to_owned(), now.to_owned()))
                }
                _ => None,
            },
        );
    let note = match (changes, accepted) {
        _ if moved.is_some() => moved.as_ref().map(|(wrote, now)| DecideNote {
            text: format!(
                "Your note was written about {wrote}; {now} has arrived. Send it on {now}, edit it, or cancel."
            ),
            hue: Hue::Held,
        }),
        (Some((sent, ActStatus::Done)), _) => Some(DecideNote {
            text: format!("Changes requested on {}", sent.revision),
            hue: Hue::Ok,
        }),
        (Some((_, status)), _) => Some(status_note("Request changes", &status)),
        (None, Some((_, status))) if status != ActStatus::Done => {
            Some(status_note("Accept", &status))
        }
        _ => gate.as_ref().err().cloned(),
    };
    let attribution: Rich = match (&note, phone, detail.revision.as_deref()) {
        (None, false, Some(rev)) => {
            let mut words = vec![
                span("Runs "),
                span(format!("plan accept {id} {rev}")).mono(),
                span(" after a fresh confirm"),
            ];
            if let Some(via) = via(feed) {
                words.push(span("\n"));
                words.push(span(format!("actor=owner · {via}")).mono());
            }
            words
        }
        _ => vec![],
    };
    Decide {
        buttons,
        compose,
        note,
        attribution,
    }
}

/// The awaiting revision: needs-you head, document, decisions, and the brief beside them.
pub(super) fn awaiting(
    state: &State,
    feed: &Feed,
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    stale: Option<String>,
    env: Env,
) -> AwaitingView {
    let phone = env.form == Form::Phone;
    let rev = detail.revision.clone();
    let members = detail.document.as_ref().map_or(0, |doc| doc.proposed.len());
    let review = rev.as_deref().and_then(|rev| {
        detail
            .reviews
            .iter()
            .rev()
            .find(|review| review.revision == rev)
    });
    let review_chip = match (review, rev.as_deref()) {
        (Some(review), Some(rev)) => Chip {
            label: format!("reviewed · {} {rev}", review.verdict),
            hue: if review_mark(&review.verdict) == Mark::Done {
                Hue::Ok
            } else {
                Hue::Held
            },
        },
        (_, Some(rev)) => Chip {
            label: format!("not reviewed · {rev}"),
            hue: Hue::Held,
        },
        _ => Chip {
            label: "revision unknown".into(),
            hue: Hue::Held,
        },
    };
    let mut sections = sections(detail.document.as_ref(), detail, feed);
    if !detail.owner_decisions.is_empty() {
        let locked = in_flight(state.live.accept.as_ref(), feed, id);
        sections.push(DocumentSection {
            heading: "Owner decisions · resolve to accept",
            body: SectionBody::Decisions(
                detail
                    .owner_decisions
                    .iter()
                    .map(|decision| DecisionRow {
                        plan: id.clone(),
                        decision: decision.clone(),
                        resolution: state.live.resolution_of(id, decision).to_owned(),
                        locked,
                    })
                    .collect(),
            ),
        });
    }
    let delivery = row.and_then(|row| row.delivery.clone());
    sections.push(DocumentSection {
        heading: "What accepting does",
        body: SectionBody::Paragraph(match delivery {
            Some(delivery) => vec![
                span(format!("Publishes {members} member tasks to ")),
                span(delivery).bold(),
                span(" delivery. Gates, independent review and landing checks stay unchanged."),
            ],
            None => vec![span(format!(
                "Publishes {members} member tasks. Gates, independent review and landing checks stay unchanged."
            ))],
        }),
    });
    let decide = decide(state, feed, id, detail, env);
    let brief = super::awaiting_brief::brief(feed, detail, &mut sections, decide);
    AwaitingView {
        header: Header {
            plan: Some(id.clone()),
            chips: vec![state_chip(WireState::Awaiting), review_chip],
            id: rev.map_or_else(|| id.to_string(), |rev| format!("{id} {rev}")),
            project: project_of(row),
            title: title(id, row),
            meta: vec![],
            illustrative: None,
            actions: if phone {
                diff_toggle(state, id, detail).into_iter().collect()
            } else {
                vec![]
            },
            stale,
        },
        sections,
        brief,
    }
}

/// An executing or approved plan's document, with the way back.
pub(super) fn document_view(
    state: &State,
    feed: &Feed,
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    stale: Option<String>,
) -> DocumentView {
    let mut actions = vec![Button::new(
        "Close document",
        Action::Live(LiveAction::Overview(id.clone())),
    )];
    actions.extend(diff_toggle(state, id, detail));
    DocumentView {
        header: Header {
            plan: Some(id.clone()),
            chips: vec![state_chip(detail.state)],
            id: detail
                .revision
                .as_ref()
                .map_or_else(|| id.to_string(), |rev| format!("{id} {rev}")),
            project: project_of(row),
            title: title(id, row),
            meta: vec![],
            illustrative: None,
            actions,
            stale,
        },
        sections: sections(detail.document.as_ref(), detail, feed),
    }
}

fn classify(line: &str) -> DiffLine {
    let kind = if line.starts_with("+++")
        || line.starts_with("---")
        || line.starts_with("@@")
        || line.starts_with("diff ")
    {
        DiffLineKind::Header
    } else if line.starts_with('+') {
        DiffLineKind::Added
    } else if line.starts_with('-') {
        DiffLineKind::Removed
    } else {
        DiffLineKind::Context
    };
    DiffLine {
        text: line.to_owned(),
        kind,
    }
}

/// The fully assembled diff between two revisions, with the earlier revisions to compare
/// against and, while the plan awaits acceptance, its decide bar.
#[allow(clippy::too_many_arguments)]
pub(super) fn diff(
    state: &State,
    feed: &Feed,
    id: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    from: &str,
    to: &str,
    env: Env,
) -> DiffView {
    let target = DetailRef::PlanDiff {
        id: id.to_string(),
        from: from.to_owned(),
        to: to.to_owned(),
    };
    let cache = feed.detail(&target);
    let mut lines = vec![];
    let mut truncated = false;
    for body in cache.into_iter().flat_map(|cache| cache.bodies()) {
        if let DetailBody::PlanDiff(page) = body {
            lines.extend(page.patch.lines().map(classify));
            truncated = page.truncated;
        }
    }
    let loading = cache.is_none_or(|cache| cache.loading);
    let note = match cache.and_then(|cache| cache.error.as_ref()) {
        Some(error) => Some(refused(format!(
            "corrald could not send the diff: {}",
            error.message
        ))),
        None if loading && lines.is_empty() => Some(DecideNote {
            text: "Loading the diff from corrald…".into(),
            hue: Hue::Neutral,
        }),
        None if loading => Some(DecideNote {
            text: "Loading more…".into(),
            hue: Hue::Neutral,
        }),
        None if lines.is_empty() => Some(DecideNote {
            text: format!("No differences between {from} and {to}."),
            hue: Hue::Neutral,
        }),
        None if truncated => Some(waiting("corrald cut this diff at its size bound.")),
        None => None,
    };
    let mut actions: Vec<Button> = diff_toggle(state, id, detail).into_iter().collect();
    actions.extend(earlier(detail).into_iter().map(|rev| {
        let mut choice = Button::new(
            format!("from {rev}"),
            Action::Live(LiveAction::DiffFrom {
                plan: id.clone(),
                from: rev.clone(),
            }),
        )
        .quiet();
        choice.enabled = rev != from;
        choice
    }));
    DiffView {
        header: Header {
            plan: Some(id.clone()),
            chips: vec![Chip {
                label: "revision diff".into(),
                hue: Hue::Neutral,
            }],
            id: format!("{id} {from} → {to}"),
            project: project_of(row),
            title: title(id, row),
            meta: vec![],
            illustrative: None,
            actions,
            stale: cache.and_then(|cache| stale_line(cache, feed)),
        },
        lines,
        note,
        more: None,
        decide: (detail.state == WireState::Awaiting).then(|| decide(state, feed, id, detail, env)),
    }
}

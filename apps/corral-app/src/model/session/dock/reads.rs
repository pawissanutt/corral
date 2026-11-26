//! What the dock reads and what comes back: reading the cards behind the host's briefs (once,
//! and again when the briefs name cards the last read did not hold), where that read stands, why
//! the cards cannot be answered from here now, and each settled reply to a request this page
//! sent, read once, after which drafts, opened cards and modes of cards the host no longer waits
//! on are forgotten.

use std::collections::BTreeSet;

use corral_protocol::detail::{CardKind, DetailGet};
use corral_protocol::session_host::{SessionActionDone, SessionCardAnswered};
use corral_protocol::session_state::CardBrief;

use crate::model::Id;
use crate::model::daemon::{Feed, Link, Operation};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::session::read_state::ReadState;
use crate::model::session::{Action, Cx, Focus, Host, State, Ui};
use crate::model::session::{notices, queue};

use super::{CardsRead, DockAction, Pending, card, fold, kind, refusal, target};

/// Reads the cards behind the briefs `ids`.
pub(super) fn read(state: &mut State, session: Id, ids: Vec<String>, feed: &Feed) -> Vec<Effect> {
    let target = target(session.as_str());
    let etag = feed
        .detail(&target)
        .and_then(|cache| cache.pages.first())
        .map(|page| page.etag.clone());
    state.ui_mut(&session).dock.asked = Some(ids);
    vec![Effect::Detail(DetailGet {
        target,
        etag,
        page: None,
    })]
}

/// Follows the feed for one page's dock: reads each settled reply once, opens the exit choice
/// Finish… asked for once it docks, and forgets drafts, opened cards and modes of cards the host
/// no longer waits on.
pub(in crate::model::session) fn settle(ui: &mut Ui, scope: &str, feed: &Feed) {
    let settled: Vec<(Id, Pending)> = ui
        .dock
        .pending
        .iter()
        .filter(|(key, _)| {
            feed.operations
                .get(*key)
                .is_some_and(|operation| !operation.loading)
        })
        .map(|(key, pending)| (key.clone(), pending.clone()))
        .collect();
    let mut approval_waits = Vec::new();
    let mut finished_reviews = BTreeSet::new();
    let mut outcomes = Vec::new();
    for (key, pending) in settled {
        ui.dock.pending.remove(&key);
        let Some(operation) = feed.operations.get(&key) else {
            continue;
        };
        match pending {
            Pending::Card { card } => {
                let kind = kind(feed, scope, &card);
                let why = refusal(operation);
                if let Some(why) = why.as_ref().filter(|why| approval_wait(kind, why)) {
                    if let Some(attempt) = ui.dock.plan_attempts.get_mut(&card) {
                        attempt.acknowledged = true;
                        attempt.duplicate = true;
                        attempt.probe_needed = true;
                    }
                    approval_waits.push((card, format!("Answer not taken: {why}")));
                    continue;
                }
                let reply = operation.decode::<SessionCardAnswered>();
                let status = reply
                    .as_ref()
                    .map(|reply| reply.status.clone())
                    .unwrap_or_default();
                if (status == "preparing" || status == "waiting") && why.is_none() {
                    if let Some(attempt) = ui.dock.plan_attempts.get_mut(&card) {
                        attempt.acknowledged = true;
                        attempt.ack_epoch = reply
                            .as_ref()
                            .and_then(|reply| reply.approval_epoch.clone());
                        attempt.ack_host =
                            reply.as_ref().and_then(|reply| reply.approval_host.clone());
                        attempt.duplicate = status == "waiting";
                        attempt.probe_needed = true;
                    }
                    if status == "waiting" {
                        approval_waits.push((
                            card,
                            "Answer not taken: This review is already being answered; wait for it"
                                .into(),
                        ));
                        continue;
                    }
                } else if kind == Some(CardKind::PlanReview)
                    || ui.dock.plan_attempts.contains_key(&card)
                {
                    ui.dock.plan_attempts.remove(&card);
                    finished_reviews.insert(card.clone());
                }
                if let Some(why) = why {
                    ui.notices
                        .push(Hue::Fail, format!("Answer not taken: {why}"), feed.now);
                } else {
                    answered(ui, card, kind, status, feed.now);
                }
            }
            Pending::Probe { card, baseline } => {
                if let Some(result) =
                    settle_probe(ui, scope, &key, &card, baseline, operation, feed)
                {
                    if result.finished {
                        finished_reviews.insert(card);
                    }
                    outcomes.extend(result.outcome);
                }
            }
            Pending::Action { what } => match refusal(operation) {
                Some(why) => ui
                    .notices
                    .push(Hue::Fail, format!("{what} refused: {why}"), feed.now),
                None => {
                    if let Some(text) = operation
                        .decode::<SessionActionDone>()
                        .and_then(|done| done.text)
                    {
                        ui.notices.push(Hue::Neutral, text, feed.now);
                    }
                }
            },
            pending => queue::settled(ui, pending, operation, feed.now),
        }
    }
    let state = feed.live.get(scope).and_then(|live| live.state.as_ref());
    if let Some(state) = state {
        ui.dock.plan_attempts.retain(|card, attempt| {
            let open = state.cards.iter().any(|brief| &brief.id == card);
            let plan = state.plan_mode.as_ref();
            let preparing = plan.is_some_and(|plan| plan.preparing.is_some());
            let pending = plan.and_then(|plan| plan.approval_pending).unwrap_or(preparing);
            let current_epoch = plan.and_then(|plan| plan.approval_epoch.as_deref());
            let current_host = plan.and_then(|plan| plan.approval_host.as_deref());
            let correlated = attempt.ack_host.as_deref().zip(attempt.ack_epoch.as_deref())
                .zip(current_host.zip(current_epoch)).is_some_and(|((host, epoch), (current_host, current_epoch))| {
                    host == current_host && epoch.parse::<u64>().ok().zip(current_epoch.parse::<u64>().ok())
                        .is_some_and(|(expected, current)| current >= expected)
                });
            // A different live host is not ordered against the ACK; only the post-ACK probe can settle it.
            let legacy_fresh = attempt.ack_epoch.is_none() && attempt.initial_epoch.as_ref()
                .zip(current_epoch).is_some_and(|(old, current)| old != current);
            let failed = attempt.ack_epoch.is_none() && state.notices.iter().any(|notice|
                notices::is_plan_failure(notice) && !attempt.prior_failures.contains(&notice.id));
            if preparing && open { attempt.observed = true; }
            let authoritative = correlated;
            let same_pending = pending && attempt.ack_host.as_deref() == current_host
                && attempt.ack_epoch.as_deref() == current_epoch;
            if same_pending {
                attempt.probe_unconfirmed = false;
                if attempt.probe_id.is_none() && !attempt.probe_needed {
                    attempt.last_probe_plan = plan.cloned();
                }
            }
            if attempt.acknowledged && attempt.probe_id.is_none() && !attempt.probe_needed
                && attempt.last_probe_plan.as_ref() != plan && !same_pending {
                attempt.probe_needed = true;
                attempt.probe_failed = false;
            }
            let history = if attempt.ack_host.as_deref() == current_host {
                plan.and_then(|plan| plan.approval_failures.iter().find(|failure| Some(failure.epoch.as_str()) == attempt.ack_epoch.as_deref()))
                    .map(|failure| failure.error.as_str())
            } else { None };
            if attempt.acknowledged && pending && history.is_some() {
                if let (Some(host), Some(epoch), Some(error)) = (attempt.ack_host.as_deref(), attempt.ack_epoch.as_deref(), history) {
                    outcomes.push((Some(notices::plan_failure_id(host, epoch)), error.to_owned()));
                    finished_reviews.insert(card.clone());
                }
                attempt.ack_host = current_host.map(str::to_owned);
                attempt.ack_epoch = current_epoch.map(str::to_owned);
                attempt.duplicate = true;
                attempt.probe_needed = true;
                attempt.probe_failed = false;
                return true;
            }
            // A scheduled or in-flight post-ACK read must settle before an unordered live frame can.
            if attempt.probe_id.is_some() || attempt.probe_needed || attempt.probe_failed {
                if attempt.acknowledged {
                    if let (Some(host), Some(epoch), Some(error)) = (attempt.ack_host.as_deref(), attempt.ack_epoch.as_deref(), history) {
                        outcomes.push((Some(notices::plan_failure_id(host, epoch)), error.to_owned()));
                        finished_reviews.insert(card.clone());
                    }
                }
                // The fresh read, not an unordered live frame, settles the guard.
                return true;
            }
            let complete = attempt.acknowledged && (history.is_some() || (authoritative && !pending)
                || (attempt.ack_epoch.is_none() && (failed || (legacy_fresh && !pending)
                    || (attempt.initial_epoch.is_none() && attempt.observed && !pending))));
            if complete { finished_reviews.insert(card.clone()); }
            if open && complete && (authoritative || legacy_fresh || history.is_some()) {
                let ack = attempt.ack_host.as_deref().zip(attempt.ack_epoch.as_deref());
                let exact = ack.is_some_and(|pair| Some(pair) == current_host.zip(current_epoch));
                let error = history.or_else(|| (exact || legacy_fresh)
                    .then(|| plan.and_then(|plan| plan.approval_error.as_deref())).flatten());
                let notice_id = if error.is_some() && (history.is_some() || exact) {
                    ack.map(|(host, epoch)| notices::plan_failure_id(host, epoch))
                } else { None };
                outcomes.push((notice_id, error.unwrap_or("Preparation ended; its outcome is unavailable. Review the plan before retrying.").to_owned()));
            }
            !complete
        });
        // A bounded live snapshot may omit cards; retain the proof until a newer epoch
        // from the probed host supersedes it.
        ui.dock.resolved_probes.retain(|_, resolution| {
            super::resolution_view(resolution, state.plan_mode.as_ref())
                != super::ResolutionView::Newer
        });
    }
    // The original answer may settle in the same pass as a duplicate, before the host's
    // live state catches up. Never raise (or keep) the old wait over that outcome.
    let pending = &ui.dock.pending;
    let waiting = |card: &str| {
        !finished_reviews.contains(card)
            && (ui.dock.plan_attempts.contains_key(card) || pending.values().any(
                |pending| matches!(pending, Pending::Card { card: answering } if answering == card),
            ) || (super::live_busy(&ui.dock, state, card) && state.is_some_and(|state| {
                state.cards.iter().any(|brief| brief.kind == CardKind::PlanReview && brief.id == card)
            })))
    };
    ui.notices.retain_approval_wait(waiting);
    ui.notices.observe_plan_failures(state, feed.now);
    for (notice_id, outcome) in outcomes {
        if let Some(notice_id) = notice_id {
            ui.notices
                .record_plan_failure(&notice_id, Hue::Held, outcome, feed.now);
        } else {
            ui.notices.push(Hue::Held, outcome, feed.now);
        }
    }
    for (card, text) in approval_waits {
        if waiting(&card) {
            ui.notices.push_approval_wait(card, text, feed.now);
        }
    }
    let Some(state) = state else {
        return;
    };
    let open: BTreeSet<&str> = state.cards.iter().map(|brief| brief.id.as_str()).collect();
    ui.dock
        .drafts
        .retain(|card, _| open.contains(card.as_str()));
    let advertised = feed
        .assistants
        .get(scope)
        .and_then(|row| row.question_id.as_deref());
    fold::settle(ui, state, advertised, feed.now);
    ui.dock
        .answered
        .retain(|card, _| open.contains(card.as_str()));
    if ui
        .dock
        .open
        .as_deref()
        .is_some_and(|card| !open.contains(card))
    {
        ui.dock.open = None;
    }
    if ui
        .dock
        .confirming
        .as_ref()
        .is_some_and(|chosen| !open.contains(chosen.card.as_str()))
    {
        ui.dock.confirming = None;
    }
    if ui
        .dock
        .compose
        .as_ref()
        .is_some_and(|mode| !open.contains(mode.card()))
    {
        ui.dock.compose = None;
        ui.notices.push(
            Hue::Held,
            "That choice no longer waits; your text stays in the draft",
            feed.now,
        );
    }
}

struct ProbeResult {
    finished: bool,
    outcome: Option<(Option<String>, String)>,
}

/// A probe is a fresh post-ACK host read, unlike an asynchronously delivered live snapshot.
/// Discard a result if the live plan changed while the read was in flight and probe again.
fn settle_probe(
    ui: &mut Ui,
    scope: &str,
    key: &Id,
    card: &str,
    baseline: Option<corral_protocol::session_state::PlanModeBrief>,
    operation: &Operation,
    feed: &Feed,
) -> Option<ProbeResult> {
    let mut attempt = ui.dock.plan_attempts.remove(card)?;
    if attempt.probe_id.as_ref() != Some(key) {
        ui.dock.plan_attempts.insert(card.to_owned(), attempt);
        return None;
    }
    attempt.probe_id = None;
    let live = feed
        .live
        .get(scope)
        .and_then(|live| live.state.as_ref())
        .and_then(|state| state.plan_mode.clone());
    let status = operation
        .decode::<SessionActionDone>()
        .and_then(|done| done.plan_status)
        .filter(|status| {
            status.card == card
                && (!status.pending || status.open)
                && !status.host.is_empty()
                && status.host.len() <= 160
                && status.epoch.parse::<u64>().is_ok()
        });
    let same_pending = status.as_ref().is_some_and(|status| {
        status.pending
            && live.as_ref().is_some_and(|plan| {
                plan.approval_host.as_deref() == Some(status.host.as_str())
                    && plan.approval_epoch.as_deref() == Some(status.epoch.as_str())
                    && (plan.approval_pending == Some(true) || plan.preparing.is_some())
            })
    });
    if live != baseline && !same_pending {
        // The host may have returned our immutable failure even though its open/pending
        // snapshot was superseded by a live frame before the response arrived.
        let outcome = status
            .as_ref()
            .and_then(|status| correlated_failure(&attempt, status));
        let finished = outcome.is_some();
        attempt.probe_needed = true;
        ui.dock.plan_attempts.insert(card.to_owned(), attempt);
        return Some(ProbeResult { finished, outcome });
    }
    let Some(status) = status else {
        let why =
            refusal(operation).unwrap_or_else(|| "the host did not return review status".into());
        attempt.probe_failed = true;
        ui.dock.plan_attempts.insert(card.to_owned(), attempt);
        ui.notices.status_issue(
            card,
            format!("Could not check review status: {why}. Retry status to continue."),
            feed.now,
        );
        return None;
    };
    // A counter from the same host cannot go backwards. Never accept an older reply.
    if attempt.ack_host.as_deref() == Some(status.host.as_str())
        && attempt
            .ack_epoch
            .as_deref()
            .and_then(|epoch| epoch.parse::<u64>().ok())
            .zip(status.epoch.parse::<u64>().ok())
            .is_some_and(|(ack, current)| current < ack)
    {
        attempt.probe_failed = true;
        ui.dock.plan_attempts.insert(card.to_owned(), attempt);
        ui.notices.status_issue(
            card,
            "The host returned an older review status. Retry status to continue.",
            feed.now,
        );
        return None;
    }
    ui.notices.clear_status_issue(card);
    let known_failure = attempt
        .ack_host
        .as_deref()
        .zip(attempt.ack_epoch.as_deref())
        .is_some_and(|(host, epoch)| {
            let id = notices::plan_failure_id(host, epoch);
            ui.notices.has_plan_failure(&id)
                || feed
                    .live
                    .get(scope)
                    .and_then(|live| live.state.as_ref())
                    .is_some_and(|state| {
                        state.notices.iter().any(|notice| {
                            notice.id == id && notices::active_plan_failure(notice, feed.now)
                        })
                    })
        });
    let original = correlated_failure(&attempt, &status);
    if status.pending {
        let had_failure = original.is_some() || known_failure;
        if attempt.context != "probe"
            && attempt.ack_host.as_deref() != Some(status.host.as_str())
            && !known_failure
        {
            ui.notices.push_approval_wait(
                card.to_owned(),
                "The previous approval outcome is unavailable; waiting for the current answer.",
                feed.now,
            );
        }
        ui.dock.resolved_probes.remove(card);
        attempt.probe_unconfirmed = !same_pending;
        attempt.ack_host = Some(status.host);
        attempt.ack_epoch = Some(status.epoch);
        attempt.last_probe_plan = live;
        attempt.probe_failed = false;
        attempt.probe_needed = false;
        ui.dock.plan_attempts.insert(card.to_owned(), attempt);
        return Some(ProbeResult {
            finished: had_failure,
            outcome: original,
        });
    }
    if !ui.dock.resolved_probes.contains_key(card)
        && ui.dock.resolved_probes.len() >= super::MAX_PLAN_ATTEMPTS
        && let Some(evicted) = ui.dock.resolved_probes.keys().next().cloned()
    {
        ui.dock.resolved_probes.remove(&evicted);
    }
    ui.dock.resolved_probes.insert(
        card.to_owned(),
        super::ProbeResolution {
            baseline,
            host: status.host,
            epoch: status.epoch,
            open: status.open,
        },
    );
    let outcome =
        original.or_else(|| {
            (status.open && !known_failure && attempt.context != "probe").then(|| {
                (None,
        "Preparation ended; its outcome is unavailable. Review the plan before retrying.".into())
            })
        });
    Some(ProbeResult {
        finished: true,
        outcome,
    })
}

fn correlated_failure(
    attempt: &super::PlanAttempt,
    status: &corral_protocol::session_host::PlanStatus,
) -> Option<(Option<String>, String)> {
    if attempt.ack_host.as_deref() != Some(status.host.as_str()) {
        return None;
    }
    let epoch = attempt.ack_epoch.as_deref()?;
    let expected = epoch.parse::<u64>().ok()?;
    if status.epoch.parse::<u64>().ok()? < expected {
        return None;
    }
    status
        .failures
        .iter()
        .find(|failure| failure.epoch == epoch)
        .map(|failure| {
            (
                Some(notices::plan_failure_id(&status.host, epoch)),
                failure.error.clone(),
            )
        })
        .or_else(|| {
            (status.epoch == epoch)
                .then(|| status.error.clone())
                .flatten()
                .map(|error| (Some(notices::plan_failure_id(&status.host, epoch)), error))
        })
}

// corrald forwards the reason, not the Pi refusal's code. The review-specific reason still
// identifies a late duplicate after the card is gone; the bridge's shared-card guard does not.
fn approval_wait(kind: Option<CardKind>, why: &str) -> bool {
    why == "This review is already being answered; wait for it"
        || (kind == Some(CardKind::PlanReview) && why == "the card is already being answered")
}

fn answered(ui: &mut Ui, card: String, kind: Option<CardKind>, status: String, now: u64) {
    match status.as_str() {
        "expired" => ui.notices.push(
            Hue::Held,
            "That card expired before the answer reached the host",
            now,
        ),
        "missing" => ui.notices.push(
            Hue::Held,
            "That card no longer waits; nothing was answered",
            now,
        ),
        "answered" => {
            ui.dock.answered.insert(card.clone(), status);
        }
        _ => {}
    }
    // A plan review's answer is taken before it is carried out: an execute that fails afterwards
    // leaves the same card waiting, so its note stays until the host no longer briefs the card.
    if kind != Some(CardKind::PlanReview) {
        ui.dock.drafts.remove(&card);
    }
    if ui.dock.open.as_deref() == Some(card.as_str()) {
        ui.dock.open = None;
        ui.focus = Focus::Composer;
    }
}

/// Why the cards cannot be answered from here now.
pub(super) fn inert(cx: &Cx<'_>) -> Option<String> {
    if cx.feed.link != Link::Up {
        return Some("answer when corrald is back".into());
    }
    match cx.host {
        Host::NotRunning | Host::Unreachable | Host::Ended => {
            Some("the host must be running to take an answer".into())
        }
        Host::Running | Host::Unknown => None,
    }
}

/// The cards to read now: once while none was read, and again when the briefs name cards the
/// last read did not hold. A failed read is retried from the dock, not here.
pub(in crate::model::session) fn fetch(cx: &Cx<'_>) -> Option<Action> {
    let briefs = cx
        .state
        .map(|state| state.cards.as_slice())
        .unwrap_or_default();
    if briefs.is_empty() || cx.host != Host::Running {
        return None;
    }
    let cache = cx.feed.detail(&target(cx.id.as_str()));
    if cache.is_some_and(|cache| cache.loading) {
        return None;
    }
    let ids: Vec<String> = briefs.iter().map(|brief| brief.id.clone()).collect();
    let missing = briefs
        .iter()
        .any(|brief| card(cx.feed, cx.id.as_str(), &brief.id).is_none());
    let wanted = cache.is_none() || (missing && cx.ui.dock.asked.as_ref() != Some(&ids));
    wanted.then(|| {
        Action::Dock(DockAction::Read {
            session: cx.id.clone(),
            ids,
        })
    })
}

/// Where the read of briefed cards whose bodies are not here stands.
pub(super) fn cards_read(cx: &Cx<'_>, briefs: &[CardBrief]) -> Option<CardsRead> {
    briefs
        .iter()
        .any(|brief| card(cx.feed, cx.id.as_str(), &brief.id).is_none())
        .then(|| card_read(cx))
}

/// Where the read stands of a briefed card whose body is not here: reading while a read is under
/// way or about to be asked for, the failure with Retry, and, once a read came back without the
/// card, that it was not there, with Retry. Nothing is inferred meanwhile.
pub(in crate::model::session) fn card_read(cx: &Cx<'_>) -> CardsRead {
    let target = target(cx.id.as_str());
    let read = ReadState::of(cx.feed.detail(&target), cx.feed);
    let retry = Some(Action::Refresh(target));
    match read {
        ReadState::Failed(_) => CardsRead {
            words: read.words(),
            hue: read.hue(),
            retry,
        },
        ReadState::Read { .. } | ReadState::Stale { .. } if fetch(cx).is_none() => CardsRead {
            words: "! not in the last read of the cards · nothing is inferred".into(),
            hue: Hue::Held,
            retry,
        },
        _ => CardsRead {
            words: "reading…".into(),
            hue: Hue::Neutral,
            retry: None,
        },
    }
}

#[cfg(test)]
#[path = "reads_tests.rs"]
mod tests;

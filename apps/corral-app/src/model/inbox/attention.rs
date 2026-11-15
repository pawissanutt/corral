//! One canonical owner-attention set for destination badges, groups and Inbox content.
use corral_protocol::rows::{AttentionActor, AttentionCounts, InboxKind, InboxRow};

use crate::model::daemon::Feed;
use crate::model::hue::Hue;

/// Canonical open owner moves; PM work never becomes an owner badge.
pub fn owner_moves(feed: &Feed) -> impl Iterator<Item = &InboxRow> {
    feed.inbox.values().filter(|row| super::is_open(row))
}

/// Apply the wire classifier without cloning rows or inferring attention from board status.
pub fn attention_counts(feed: &Feed) -> AttentionCounts {
    let mut counts = AttentionCounts::default();
    for row in feed
        .inbox
        .values()
        .filter(|row| row.kind != InboxKind::PmAction)
    {
        let one = AttentionCounts::from_rows(std::slice::from_ref(row));
        counts.total += one.total;
        counts.inbox += one.inbox;
        counts.now += one.now;
        counts.plans += one.plans;
        counts.review += one.review;
    }
    counts
}

/// Why a move is waiting, what happens next, and an informational CLI hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionLine {
    pub whose: String,
    pub hue: Hue,
    pub reason: String,
    pub next_label: &'static str,
    pub next_step: String,
    pub command_label: &'static str,
    pub command: Option<String>,
}

pub fn attention_line(row: &InboxRow) -> Option<AttentionLine> {
    let attention = row.attention.as_ref()?;
    let (whose, hue) = match attention.whose_move {
        AttentionActor::Owner => ("Your move", Hue::You),
        AttentionActor::Pm => ("PM's move", Hue::Corral),
        AttentionActor::Worker => ("Worker's move", Hue::Worker),
        AttentionActor::External => ("Waiting externally", Hue::Held),
        AttentionActor::Other => ("Next mover unknown", Hue::Neutral),
    };
    Some(AttentionLine {
        whose: whose.into(),
        hue,
        reason: attention.reason.clone(),
        next_label: "Next step",
        next_step: attention.next_step.clone(),
        command_label: "Command hint",
        command: attention.command.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::health::{self, Snapshot};
    use crate::model::nav::{Destination, Form};
    use corral_protocol::rows::{AttentionDestination, AttentionMove, InboxKind, InboxState};

    fn row(
        id: &str,
        kind: InboxKind,
        actor: AttentionActor,
        destination: AttentionDestination,
    ) -> InboxRow {
        InboxRow {
            id: id.into(),
            kind,
            task: Some(id.into()),
            plan: None,
            project: None,
            title: id.into(),
            from: String::new(),
            asked_at: None,
            needed_by: None,
            blocking: false,
            state: InboxState::Open,
            question_rev: None,
            answerable: false,
            unanswerable_reason: None,
            rev: 1,
            attention: Some(AttentionMove {
                whose_move: actor,
                reason: "blocker".into(),
                next_step: "decide".into(),
                command: None,
                destination,
            }),
        }
    }

    #[test]
    fn all_destination_badges_share_owner_moves_and_never_count_pm_work() {
        let mut feed = Feed::default();
        for row in [
            row(
                "question",
                InboxKind::PmAsk,
                AttentionActor::Owner,
                AttentionDestination::Inbox,
            ),
            row(
                "plan",
                InboxKind::PlanAcceptance,
                AttentionActor::Owner,
                AttentionDestination::Plans,
            ),
            row(
                "review",
                InboxKind::Review,
                AttentionActor::Owner,
                AttentionDestination::Review,
            ),
            row(
                "pm",
                InboxKind::PmAction,
                AttentionActor::Pm,
                AttentionDestination::Now,
            ),
            row(
                "malformed-pm",
                InboxKind::PmAction,
                AttentionActor::Owner,
                AttentionDestination::Now,
            ),
        ] {
            feed.inbox.insert(row.id.clone().into(), row);
        }
        let mut closed = row(
            "closed",
            InboxKind::OwnerDecision,
            AttentionActor::Owner,
            AttentionDestination::Now,
        );
        closed.state = InboxState::Cancelled;
        feed.inbox.insert(closed.id.clone().into(), closed);
        let mut legacy = row(
            "legacy",
            InboxKind::Stuck,
            AttentionActor::Owner,
            AttentionDestination::Now,
        );
        legacy.attention = None;
        feed.inbox.insert(legacy.id.clone().into(), legacy);
        let expected = AttentionCounts {
            total: 3,
            inbox: 3,
            now: 3,
            plans: 1,
            review: 1,
        };
        assert_eq!(attention_counts(&feed), expected);
        assert_eq!(super::super::ordered(&feed).len(), 3);
        let env = health::daemon_env(&feed, Form::Terminal);
        let list = super::super::live_list::list(&super::super::State::default(), &feed, env);
        let super::super::InboxList::Ready {
            needs_count, items, ..
        } = list
        else {
            panic!("attention rows are already loaded");
        };
        assert_eq!(needs_count, expected.inbox as usize);
        assert_eq!(items.len(), needs_count);
        assert_eq!(health::needs_you(&feed), 3);
        assert_eq!(health::awaiting_plans(&feed), 1);
        let snapshot: Snapshot = health::daemon_snapshot(&feed, Form::Terminal);
        for (destination, count) in [
            (Destination::Inbox, 3),
            (Destination::Now, 3),
            (Destination::Plans, 1),
            (Destination::Review, 1),
        ] {
            assert_eq!(snapshot.count(destination), Some(count));
        }
    }

    #[test]
    fn blocker_context_retains_the_canonical_reason_next_step_and_command_hint() {
        let mut row = row(
            "decision",
            InboxKind::OwnerDecision,
            AttentionActor::Owner,
            AttentionDestination::Now,
        );
        row.attention.as_mut().expect("canonical move").command =
            Some("corral plan advance T-1".into());
        let line = attention_line(&row).expect("attention context");
        let source = row.attention.as_ref().expect("canonical move");
        assert_eq!(line.reason, source.reason);
        assert_eq!(line.next_step, source.next_step);
        assert_eq!(line.command, source.command);
    }
}

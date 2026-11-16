//! The sample fleet of one previewed worker: what corrald's live page carries for a watched
//! session that runs child agents, applied through the live model as the client applies a
//! daemon's page. Other previewed sessions have none, and show none.

use corral_protocol::subscription::{
    SessionAgentMessage, SessionChild, SessionChildVerdict, SessionFleet, SessionLiveEvent,
    SessionLiveKind,
};

use super::stamp;
use crate::model::daemon::Feed;

/// The previewed session that runs children.
const SESSION: &str = "T-871";

/// Watching `session` delivers its fleet, once the preview's clock has started.
pub(super) fn watch(feed: &mut Feed, session: &str) {
    if session != SESSION || feed.now == 0 {
        return;
    }
    let now = feed.now;
    let ms = |ago: u64| now.saturating_sub(ago).saturating_mul(1000);
    let child =
        |id: &str, kind: &str, title: &str, state: &str, (start, end): (u64, Option<u64>)| {
            SessionChild {
                id: id.into(),
                kind: kind.into(),
                title: title.into(),
                state: state.into(),
                model: "preview/gpt-6.1-terra".into(),
                started_at: Some(ms(start)),
                ended_at: end.map(ms),
                activity: None,
                tokens: 0,
                cost_usd: None,
                verdict: None,
                question: None,
            }
        };
    let message = |seq: u64, ago: u64, (from, to): (&str, &str), text: &str, delivery: &str| {
        SessionAgentMessage {
            seq,
            at: ms(ago),
            from: from.into(),
            to: to.into(),
            text: text.into(),
            delivery: delivery.into(),
        }
    };
    let fleet = SessionFleet {
        children: vec![
            SessionChild {
                activity: Some("edit".into()),
                tokens: 48_200,
                cost_usd: Some(0.61),
                ..child(
                    "reader",
                    "slice",
                    "Read annotation records without dropping the last one",
                    "running",
                    (420, None),
                )
            },
            SessionChild {
                tokens: 21_400,
                verdict: Some(SessionChildVerdict::Changes),
                ..child(
                    "review-1",
                    "review",
                    "Review the reader change",
                    "done",
                    (840, Some(540)),
                )
            },
            SessionChild {
                tokens: 9_800,
                cost_usd: Some(0.08),
                ..child(
                    "callers",
                    "explore",
                    "Find every caller of annot::record",
                    "done",
                    (1200, Some(1020)),
                )
            },
        ],
        messages: vec![
            message(
                1,
                1200,
                ("parent", "callers"),
                "List every caller of annot::record and what each expects back.",
                "delivered",
            ),
            message(
                2,
                540,
                ("review-1", "parent"),
                "Two findings: the reader drops the trailing record, and its error loses the path.",
                "delivered",
            ),
            message(
                3,
                60,
                ("parent", "T-854"),
                "This reader change touches your publish path; rebase once it lands.",
                "queued",
            ),
        ],
        messages_total: 3,
        max_parallel: 3,
    };
    let event = SessionLiveEvent {
        seq: 1,
        generation: 1,
        at: stamp(feed, now),
        kind: SessionLiveKind::LiveFleet {},
    };
    let live = feed.live.entry(SESSION.into()).or_default();
    live.apply_page(Some(1), vec![event], false);
    live.replace_fleet(Some(fleet));
}

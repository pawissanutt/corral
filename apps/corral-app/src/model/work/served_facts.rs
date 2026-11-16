//! The board's health in the Daemon state: whether its rows are last known, and the banner
//! with its Retry while the board subscription fails or refreshes.

use crate::model::daemon::{self, Link};
use crate::model::hue::Hue;
use crate::model::text::span;

use super::{Button, ButtonKind, Press, Status};

/// Board-topic failure is independent of socket health.
pub(super) fn board_problem(feed: &daemon::Feed) -> Option<String> {
    let topic = feed.topics.get("board")?;
    if let Some(error) = &topic.error {
        return Some(format!("Board unavailable: {}", error.message));
    }
    (topic.stale && !topic.loading).then(|| "The board subscription is out of date.".into())
}

/// Retained rows are last known while either the topic or its transport is not current.
pub fn daemon_status(feed: &daemon::Feed) -> Status {
    let topic = feed.topics.get("board");
    let stale = feed.link != Link::Up
        || topic.is_some_and(|topic| topic.stale || topic.loading || topic.error.is_some());
    let dimmed = stale && feed.board.complete;
    let problem = board_problem(feed);
    let pending = topic.is_some_and(|topic| topic.loading);
    Status {
        dimmed,
        as_of: dimmed.then(|| {
            feed.synced
                .as_ref()
                .filter(|clock| !clock.is_empty())
                .map_or_else(
                    || "last known board".into(),
                    |clock| format!("last known board · as of {clock}"),
                )
        }),
        banner: problem
            .or_else(|| (dimmed && pending).then(|| "Refreshing the board…".into()))
            .map(|message| super::Banner {
                hue: Hue::Held,
                text: vec![
                    span(message),
                    span(if dimmed {
                        " Showing last known rows."
                    } else {
                        ""
                    }),
                ],
                actions: vec![
                    Button::new("Retry board", Press::RetryBoard, ButtonKind::Ghost)
                        .enabled(feed.link == Link::Up && !pending),
                ],
            }),
    }
}

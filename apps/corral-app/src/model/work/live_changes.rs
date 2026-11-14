//! Changes in the Daemon state: corrald's durable change lines, newest first, split by this
//! device's own seen cursor, filtered, paged backwards, with corrald's attribution and the
//! device's Mark seen advancing its cursor through the newest line.

use crate::model::daemon::{Feed, Link};
use crate::model::nav::{Form, PhoneTab};

use super::{
    Action, Button, ButtonKind, Changes, FeedRow, Press, State, change_row, changes_loading,
    changes_notice, daemon_status, feed_filters, stamp,
};

/// When this device last saw the feed, in local time, if it ever did.
fn last_visit(feed: &Feed) -> Option<String> {
    feed.seen
        .as_ref()
        .filter(|seen| seen.through != 0)
        .and_then(|seen| seen.at.as_deref())
        .map(|at| stamp(at, feed))
        .filter(|at| !at.is_empty())
}

/// Changes in the Daemon state. `device` is the authenticated device's name, whose cursor this
/// is; `tab` decides the phone's back label.
pub fn live_changes(
    state: &State,
    feed: &Feed,
    device: Option<&str>,
    form: Form,
    tab: PhoneTab,
) -> Changes {
    let phone = form == Form::Phone;
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    let visit = last_visit(feed);
    let mut recent: Vec<FeedRow> = Vec::new();
    let mut history: Vec<FeedRow> = Vec::new();
    for line in feed
        .changes
        .values()
        .rev()
        .filter(|line| state.filter.admits_class(&line.class))
    {
        if line.seq > through {
            recent.push(change_row(line, feed, true));
        } else {
            history.push(change_row(line, feed, false));
        }
    }
    let unseen = feed.changes.range(through.saturating_add(1)..).count();
    let device_words = device.map_or_else(|| "this device".to_owned(), |name| name.to_owned());
    let seen_line = match (&visit, phone) {
        (Some(at), true) => format!("last visit {at}"),
        (Some(at), false) => format!("your last visit · {at} · seen-cursor for {device_words}"),
        (None, true) => "nothing marked seen yet".to_owned(),
        (None, false) => format!("nothing marked seen on {device_words} yet"),
    };
    let subtitle = (!phone).then(|| {
        let events = match unseen {
            1 => "1 new event".to_owned(),
            count => format!("{count} new events"),
        };
        match &visit {
            Some(at) => format!(
                "since this device's last visit {at} · {events} · the seen-cursor is per device: marking seen here leaves your other devices' marks"
            ),
            None => format!(
                "{events} · the seen-cursor is per device: marking seen here leaves your other devices' marks"
            ),
        }
    });
    let up = feed.link == Link::Up;
    let loading = changes_loading(feed);
    let head = feed
        .changes
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0)
        .max(feed.changes_head);
    let notice = changes_notice(feed)
        .map(|notice| notice.text().to_owned())
        .or_else(|| {
            (recent.is_empty() && history.is_empty())
                .then(|| format!("No {} events loaded.", state.filter.label().to_lowercase()))
        });
    Changes {
        title: "Changes",
        back_label: phone.then_some(if tab == PhoneTab::More {
            "More"
        } else {
            "Back"
        }),
        back: phone.then_some(Action::Back),
        subtitle,
        filters: feed_filters(state, form),
        recent,
        seen_line,
        history,
        illustrative: None,
        // Each device keeps its own cursor, so the phone can move its own too.
        mark_seen: feed.capable(corral_protocol::app::CHANGES_V1).then(|| {
            if head > through {
                Button::new("Mark seen", Press::Seen(head), ButtonKind::Ghost).enabled(up)
            } else {
                Button::new("All seen", Press::Seen(head), ButtonKind::Ghost).enabled(false)
            }
        }),
        older: feed.changes_older.map(|before| {
            Button::new(
                if loading {
                    "Loading older…"
                } else {
                    "Load older"
                },
                Press::Older(before),
                ButtonKind::Quiet,
            )
            .enabled(up && !loading)
        }),
        notice,
        status: daemon_status(feed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::work::ChangeFilter;
    use corral_protocol::app::{CHANGES_V1, ChangeLine, SeenCursor};

    fn line(seq: u64, class: &str) -> ChangeLine {
        ChangeLine {
            seq,
            at: "2026-11-14T10:00:00Z".into(),
            class: class.into(),
            task: Some(format!("T-{seq}")),
            text: format!("T-{seq} · {class}"),
            actor: "owner".into(),
            via: "app:phone".into(),
        }
    }

    #[test]
    fn the_seen_cursor_splits_new_from_seen_and_filters_by_class() {
        let mut feed = Feed::default();
        feed.capabilities.insert(CHANGES_V1.into());
        for (seq, class) in [(1, "gate"), (2, "question"), (3, "work"), (4, "review")] {
            feed.changes.insert(seq, line(seq, class));
        }
        feed.seen = Some(SeenCursor {
            through: 2,
            at: Some("2026-11-14T09:00:00Z".into()),
        });
        let mut state = State::default();
        let changes = live_changes(&state, &feed, Some("phone"), Form::Desktop, PhoneTab::More);
        let ids = |rows: &[FeedRow]| rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&changes.recent), ["T-4", "T-3"]);
        assert_eq!(ids(&changes.history), ["T-2", "T-1"]);
        assert_eq!(changes.recent[0].text, "review");
        assert_eq!(changes.recent[0].by.as_deref(), Some("owner via app:phone"));
        assert_eq!(
            changes.mark_seen.and_then(|button| button.action),
            Some(Press::Seen(4))
        );
        state.filter = ChangeFilter::Verdicts;
        let changes = live_changes(&state, &feed, None, Form::Desktop, PhoneTab::More);
        assert_eq!(ids(&changes.recent), ["T-4"]);
        assert_eq!(ids(&changes.history), ["T-1"]);
    }
}

//! Scoped terminal captures replace one bounded window; failures retain the last capture.
use super::*;

impl AppState {
    pub(super) fn terminal_capture(
        &mut self,
        feed: Signal<Feed>,
        scope: String,
        snapshot: Option<TerminalSnapshot>,
        error: Option<String>,
    ) {
        feed.update(|feed| apply_capture(feed, &scope, snapshot, error));
        self.touch_detail(feed, &DetailRef::Terminal { id: scope });
        self.trim_retained(feed);
    }
}

fn apply_capture(
    feed: &mut Feed,
    scope: &str,
    snapshot: Option<TerminalSnapshot>,
    error: Option<String>,
) {
    let target = DetailRef::Terminal { id: scope.into() };
    let cache = feed
        .details
        .entry(detail_key(&target))
        .or_insert_with(|| DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: vec![],
            loading: false,
            stale: true,
            error: None,
            merged: None,
        });
    let error = error.map(|message| ClientError {
        code: "unavailable".into(),
        message,
    });
    if let Some(snapshot) = snapshot {
        let as_of = snapshot.captured_at.clone();
        let body = DetailBody::Terminal(snapshot);
        cache.pages = vec![DetailReply {
            etag: format!("terminal:{as_of}"),
            as_of,
            seq: 0,
            next: None,
            body: Some(body.clone()),
        }];
        cache.merged = Some(body);
    }
    cache.loading = false;
    cache.stale = error.is_some() || cache.merged.is_none();
    cache.error = error.clone();
    feed.topics.insert(
        topic_key(Topic::Terminal, Some(scope)),
        TopicState {
            loading: false,
            stale: cache.stale,
            error,
        },
    );
    feed.synced = Some(feed.clock.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_replace_windows_and_keep_capture_time_through_failure_and_disconnect() {
        let mut feed = Feed::default();
        let target = DetailRef::Terminal {
            id: "session-a".into(),
        };
        let capture = |line: &str, at: &str| TerminalSnapshot {
            lines: vec![line.into()],
            rows: vec![],
            captured_at: at.into(),
            live: true,
        };
        apply_capture(
            &mut feed,
            "session-a",
            Some(capture("old", "2026-11-15T01:00:00Z")),
            None,
        );
        apply_capture(
            &mut feed,
            "session-a",
            Some(capture("current", "2026-11-15T01:00:01Z")),
            None,
        );
        let cache = feed.detail(&target).unwrap();
        assert_eq!(cache.pages.len(), 1);
        assert_eq!(cache.pages[0].as_of, "2026-11-15T01:00:01Z");
        assert!(
            matches!(cache.body(), Some(DetailBody::Terminal(snapshot)) if snapshot.lines == ["current"])
        );
        apply_capture(
            &mut feed,
            "session-a",
            None,
            Some("Pane unavailable".into()),
        );
        let cache = feed.detail(&target).unwrap();
        assert!(cache.stale && !cache.loading);
        assert_eq!(cache.error.as_ref().unwrap().message, "Pane unavailable");
        assert_eq!(cache.pages[0].as_of, "2026-11-15T01:00:01Z");
        assert!(feed.topics["terminal:session-a"].stale);
        feed.mark_stale();
        assert!(feed.detail(&target).unwrap().body().is_some());
        apply_capture(
            &mut feed,
            "session-a",
            Some(capture("reconnected", "2026-11-15T01:00:02Z")),
            None,
        );
        let cache = feed.detail(&target).unwrap();
        assert!(!cache.stale && cache.error.is_none());
        assert!(
            matches!(cache.body(), Some(DetailBody::Terminal(snapshot)) if snapshot.lines == ["reconnected"])
        );
        assert!(!feed.topics["terminal:session-a"].stale);
    }
}

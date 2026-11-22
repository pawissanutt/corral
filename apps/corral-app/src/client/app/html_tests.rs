use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

use crate::client::html::{HtmlFinish, HtmlSink};

use super::*;

#[derive(Debug, Default, PartialEq, Eq)]
struct Writes {
    bytes: usize,
    launched: bool,
    dropped: bool,
}

struct Sink(Arc<Mutex<Writes>>);
impl HtmlSink for Sink {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().bytes += bytes.len();
        Ok(())
    }

    fn finish(self: Box<Self>) -> HtmlFinish {
        Box::pin(async move {
            self.0.lock().unwrap().launched = true;
            Ok(())
        })
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped = true;
    }
}

fn open(
    app: &mut AppState,
    feed: Signal<Feed>,
    id: &str,
) -> (Signal<HtmlProgress>, Arc<Mutex<Writes>>, Outgoing) {
    let status = signal(HtmlProgress::Reading {
        id: id.into(),
        percent: 0,
    });
    let writes = Arc::new(Mutex::new(Writes::default()));
    let mut outgoing = app.request(
        feed,
        ClientRequest::OpenHtml(OpenHtml {
            id: id.into(),
            session: "B-1".into(),
            path: "index.html".into(),
            status,
            output: Box::new(Sink(writes.clone())),
        }),
    );
    assert_eq!(outgoing.len(), 1);
    (status, writes, outgoing.remove(0))
}

fn reply(outgoing: Outgoing, size: usize) -> Event {
    reply_with_byte(outgoing, size, b'a')
}

fn reply_with_byte((method, params, tag): Outgoing, size: usize, byte: u8) -> Event {
    let DetailRef::SessionFileChunk {
        offset, revision, ..
    } = decode::<DetailGet>(params.clone()).unwrap().target
    else {
        panic!("expected an HTML chunk request");
    };
    let rev = format!("{}.{:x}", "1".repeat(64), Sha256::digest(vec![b'a'; size]));
    assert_eq!(revision.as_deref(), (offset != 0).then_some(rev.as_str()));
    let len = (size - offset as usize).min(SESSION_HTML_CHUNK_BYTES);
    Event::Reply {
        method,
        tag,
        params,
        outcome: Ok(value(DetailReply {
            etag: "ignored".into(),
            as_of: "now".into(),
            seq: 0,
            next: None,
            body: Some(DetailBody::SessionFileChunk(SessionFileChunk {
                size: size as u64,
                offset,
                revision: rev,
                data: STANDARD.encode(vec![byte; len]),
            })),
        })),
        error_data: None,
    }
}

#[test]
fn only_full_uncancelled_snapshots_launch_and_no_chunks_enter_the_feed() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut app = AppState::new();
    let feed = signal(Feed::default());
    feed.update(|feed| {
        feed.capabilities.insert(SESSION_HTML_V1.into());
    });
    app.ready = true;
    let size = SESSION_HTML_CHUNK_BYTES + 4;

    let (status, writes, first) = open(&mut app, feed, "cancelled");
    let mut next = app.event(feed, reply(first, size)).unwrap();
    assert_eq!(next.len(), 1);
    status.set(HtmlProgress::Idle);
    app.request(feed, ClientRequest::CancelHtml("cancelled".into()));
    assert!(
        app.event(feed, reply(next.remove(0), size))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        *writes.lock().unwrap(),
        Writes {
            bytes: SESSION_HTML_CHUNK_BYTES,
            launched: false,
            dropped: true
        }
    );

    let (status, writes, first) = open(&mut app, feed, "finished");
    let mut next = app.event(feed, reply(first, size)).unwrap();
    assert_eq!(next.len(), 1);
    assert!(!writes.lock().unwrap().launched);
    assert!(
        app.event(feed, reply(next.remove(0), size))
            .unwrap()
            .is_empty()
    );
    assert_eq!(status.get_untracked(), HtmlProgress::Idle);
    assert_eq!(
        *writes.lock().unwrap(),
        Writes {
            bytes: size,
            launched: true,
            dropped: true
        }
    );
    feed.with_untracked(|feed| {
        assert!(feed.details.is_empty());
        assert!(feed.operations.is_empty());
    });
}

#[test]
fn only_matching_content_digests_launch() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut app = AppState::new();
    let feed = signal(Feed::default());
    feed.update(|feed| {
        feed.capabilities.insert(SESSION_HTML_V1.into());
    });
    app.ready = true;
    let size = SESSION_HTML_CHUNK_BYTES + 4;

    let (status, writes, first) = open(&mut app, feed, "changed");
    let mut next = app.event(feed, reply(first, size)).unwrap();
    assert_eq!(next.len(), 1);
    // Valid UTF-8 and unchanged metadata/revision, but different bytes in a later chunk.
    assert!(
        app.event(feed, reply_with_byte(next.remove(0), size, b'b'))
            .unwrap()
            .is_empty()
    );
    assert!(
        matches!(status.get_untracked(), HtmlProgress::Failed(message)
        if message.contains("could not be verified"))
    );
    assert_eq!(
        *writes.lock().unwrap(),
        Writes {
            bytes: SESSION_HTML_CHUNK_BYTES,
            launched: false,
            dropped: true,
        }
    );

    let (status, writes, first) = open(&mut app, feed, "empty");
    assert!(app.event(feed, reply(first, 0)).unwrap().is_empty());
    assert_eq!(status.get_untracked(), HtmlProgress::Idle);
    assert_eq!(
        *writes.lock().unwrap(),
        Writes {
            bytes: 0,
            launched: true,
            dropped: true,
        }
    );
}

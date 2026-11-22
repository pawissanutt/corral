use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::STANDARD};

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

fn reply((method, params, tag): Outgoing, size: usize) -> Event {
    let DetailRef::SessionFileChunk {
        offset, revision, ..
    } = decode::<DetailGet>(params.clone()).unwrap().target
    else {
        panic!("expected an HTML chunk request");
    };
    assert_eq!(revision.as_deref(), (offset != 0).then_some("v1"));
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
                revision: "v1".into(),
                data: STANDARD.encode(vec![b'a'; len]),
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

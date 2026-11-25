use std::sync::{Arc, Mutex};

use crate::client::core::Event;
use crate::client::download::{SaveFinish, SaveSink};

use super::*;

#[derive(Debug, Default, PartialEq, Eq)]
struct Writes {
    bytes: Vec<u8>,
    finished: bool,
    dropped: bool,
}

struct Sink(Arc<Mutex<Writes>>);
impl SaveSink for Sink {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn finish(self: Box<Self>) -> SaveFinish {
        Box::pin(async move {
            self.0.lock().unwrap().finished = true;
            Ok("to the test sink".into())
        })
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped = true;
    }
}

/// Serves `content` as corrald does: text in UTF-8 pages, anything else in base64 pages.
fn serve((method, params, tag): Outgoing, content: &[u8], text: bool, page: usize) -> Event {
    let request: DetailGet = decode(params.clone()).unwrap();
    let at = request
        .page
        .as_deref()
        .and_then(|token| token.rsplit_once(':'))
        .map_or(0, |(_, at)| at.parse::<usize>().unwrap());
    let mut end = (at + page).min(content.len());
    if text {
        while std::str::from_utf8(&content[at..end]).is_err() {
            end -= 1;
        }
    }
    let part = &content[at..end];
    let etag = if text { "tdigest" } else { "bdigest" };
    Event::Reply {
        method,
        tag,
        params,
        outcome: Ok(value(DetailReply {
            etag: etag.into(),
            as_of: "now".into(),
            seq: 0,
            next: (end < content.len()).then(|| format!("{etag}:{end}")),
            body: Some(DetailBody::Artifact(ArtifactPage {
                task: "T-1".into(),
                key: "2".into(),
                media_type: None,
                encoding: if text {
                    ArtifactEncoding::Text
                } else {
                    ArtifactEncoding::Base64
                },
                content: if text {
                    String::from_utf8(part.to_vec()).unwrap()
                } else {
                    STANDARD.encode(part)
                },
                truncated: end < content.len(),
            })),
        })),
        error_data: None,
    }
}

fn download(
    app: &mut AppState,
    feed: Signal<Feed>,
    content: &[u8],
    sha256: String,
    text: bool,
) -> (Signal<Saving>, Arc<Mutex<Writes>>) {
    let status = signal(Saving::Reading {
        id: "d".into(),
        percent: 0,
    });
    let writes = Arc::new(Mutex::new(Writes::default()));
    let mut outgoing = app.request(
        feed,
        ClientRequest::SaveArtifact(SaveArtifact {
            id: "d".into(),
            reference: "T-1:2".into(),
            bytes: content.len() as u64,
            sha256,
            status,
            output: Box::new(Sink(writes.clone())),
        }),
    );
    while let Some(next) = outgoing.pop() {
        outgoing = app.event(feed, serve(next, content, text, 7)).unwrap();
    }
    (status, writes)
}

#[test]
fn downloads_save_the_stored_bytes_of_text_and_binary_artifacts_apart_from_the_cache() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut app = AppState::new();
    let feed = signal(Feed::default());
    app.ready = true;
    let text = "summary — café\r\nline two\n".repeat(3).into_bytes();
    let binary = (0..=255u8).chain([0, 0xff, 0x89]).collect::<Vec<_>>();
    for (content, is_text) in [(&text, true), (&binary, false)] {
        let digest = format!("{:x}", Sha256::digest(content));
        let (status, writes) = download(&mut app, feed, content, digest, is_text);
        assert_eq!(
            status.get_untracked(),
            Saving::Saved("to the test sink".into())
        );
        let writes = writes.lock().unwrap();
        assert_eq!(writes.bytes, *content);
        assert!(writes.finished && writes.dropped);
    }
    feed.with_untracked(|feed| assert!(feed.details.is_empty()));
}

#[test]
fn bytes_that_do_not_match_the_catalog_are_never_finished() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut app = AppState::new();
    let feed = signal(Feed::default());
    app.ready = true;
    let content = b"stored bytes, three pages long".to_vec();
    let wrong = format!("{:x}", Sha256::digest(b"other"));
    let (status, writes) = download(&mut app, feed, &content, wrong, true);
    assert!(matches!(status.get_untracked(), Saving::Failed(_)));
    let writes = writes.lock().unwrap();
    assert!(!writes.finished && writes.dropped);
}

/// The preview reads a large text artifact's head, not all of it up to the cache's bound, and
/// keeps it marked truncated.
#[test]
fn a_large_text_artifact_previews_its_head_without_reading_the_rest() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut app = AppState::new();
    let feed = signal(Feed::default());
    app.ready = true;
    let content = "a line of a long log\n".repeat(20_000).into_bytes();
    let target = DetailRef::Artifact {
        reference: "T-1:2".into(),
    };
    let mut outgoing = app.request(
        feed,
        ClientRequest::Detail {
            request_id: "preview".into(),
            request: DetailGet {
                target: target.clone(),
                etag: None,
                page: None,
            },
        },
    );
    let mut reads = 0;
    while let Some(next) = outgoing.pop() {
        reads += 1;
        outgoing = app
            .event(feed, serve(next, &content, true, 8 * 1024))
            .unwrap();
    }
    assert!(reads < content.len() / (8 * 1024) / 2, "{reads} pages read");
    feed.with_untracked(|feed| {
        let Some(DetailBody::Artifact(head)) = feed.detail(&target).and_then(|cache| cache.body())
        else {
            panic!("the head was not published");
        };
        assert!(head.truncated);
        assert!(content.starts_with(head.content.as_bytes()));
    });
}

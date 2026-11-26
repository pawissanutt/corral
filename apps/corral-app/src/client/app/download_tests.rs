use std::sync::{Arc, Mutex};

use crate::client::core::Event;
use crate::client::download::{SaveFinish, SaveSink, SaveStep};

use super::*;

#[derive(Debug, Default, PartialEq, Eq)]
struct Writes {
    bytes: Vec<u8>,
    finished: bool,
    dropped: bool,
}

struct Sink(Arc<Mutex<Writes>>);
impl SaveSink for Sink {
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> SaveStep<'a> {
        self.0.lock().unwrap().bytes.extend_from_slice(bytes);
        Box::pin(std::future::ready(Ok(())))
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

/// Serves `content` as corrald does: a text stream in UTF-8 pages, except a page that is not
/// UTF-8, which travels as base64 as every page of a binary stream does.
fn serve((method, params, tag): Outgoing, content: &[u8], text: bool, page: usize) -> Event {
    let request: DetailGet = decode(params.clone()).unwrap();
    let at = request
        .page
        .as_deref()
        .and_then(|token| token.rsplit_once(':'))
        .map_or(0, |(_, at)| at.parse::<usize>().unwrap());
    let mut end = (at + page).min(content.len());
    let utf8 = match std::str::from_utf8(&content[at..end]) {
        _ if !text => false,
        Ok(_) => true,
        Err(error)
            if error.error_len().is_none() && end < content.len() && error.valid_up_to() > 0 =>
        {
            end = at + error.valid_up_to();
            true
        }
        Err(_) => false,
    };
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
                source_format: None,
                encoding: if utf8 {
                    ArtifactEncoding::Text
                } else {
                    ArtifactEncoding::Base64
                },
                content: if utf8 {
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
    // A log whose head is text, with a byte that is not UTF-8 further on.
    let mut mixed = "a long log line\n".repeat(4).into_bytes();
    mixed[40] = 0xff;
    for (content, is_text) in [(&text, true), (&binary, false), (&mixed, true)] {
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
    app.request(
        feed,
        ClientRequest::ArtifactHead {
            target: target.clone(),
            on: true,
        },
    );
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

#[derive(Debug, Default)]
struct Staged {
    bytes: Vec<u8>,
    launched: bool,
}

/// A preview that records what it was handed and whether it launched.
struct Preview(Arc<Mutex<Staged>>);
impl crate::client::html::HtmlSink for Preview {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn finish(self: Box<Self>) -> crate::client::html::HtmlFinish {
        Box::pin(async move {
            self.0.lock().unwrap().launched = true;
            Ok(())
        })
    }
}

/// Open in browser on a descriptively titled HTML report goes through the download's transfer
/// into the preview: it launches with every byte of the artifact only when the read matches the
/// catalog, and never for a head shorter than the catalog's size or for a mismatched SHA-256.
#[test]
fn open_in_browser_launches_only_the_whole_verified_artifact() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut app = AppState::new();
    let feed = signal(Feed::default());
    app.ready = true;
    let page = b"<!doctype html><title>Standalone coverage report</title><p>ok</p>".to_vec();
    let digest = format!("{:x}", Sha256::digest(&page));
    let head = digest.clone();
    for (catalog_bytes, sha256, launches) in [
        (page.len() as u64, digest, true),
        // The catalog's artifact is longer than what was served: a head, not the page.
        (page.len() as u64 + 64, head, false),
        (
            page.len() as u64,
            format!("{:x}", Sha256::digest(b"other")),
            false,
        ),
    ] {
        let status = signal(Saving::Reading {
            id: "o".into(),
            percent: 0,
        });
        let staged = Arc::new(Mutex::new(Staged::default()));
        let mut outgoing = app.request(
            feed,
            ClientRequest::SaveArtifact(SaveArtifact {
                id: "o".into(),
                reference: "T-1:2".into(),
                bytes: catalog_bytes,
                sha256,
                status,
                output: Box::new(crate::client::html::HtmlSave(Box::new(Preview(
                    staged.clone(),
                )))),
            }),
        );
        while let Some(next) = outgoing.pop() {
            outgoing = app.event(feed, serve(next, &page, true, 7)).unwrap();
        }
        let staged = staged.lock().unwrap();
        assert_eq!(staged.launched, launches, "catalog {catalog_bytes} bytes");
        if launches {
            assert_eq!(staged.bytes, page);
            assert_eq!(
                status.get_untracked(),
                Saving::Saved("in the browser".into())
            );
        } else {
            assert!(matches!(status.get_untracked(), Saving::Failed(_)));
        }
    }
}

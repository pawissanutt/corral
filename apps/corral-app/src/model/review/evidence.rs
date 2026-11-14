//! The served candidate's artifacts: catalog records, and on request their bounded contents
//! from corrald's artifact catalog (`DetailRef::Artifact`), never a host file. corrald serves
//! raw bytes without a media type, so the app tells them apart: an image by its own magic
//! bytes becomes a `data:` URI where the form can draw one, UTF-8 is shown as text without
//! terminal control sequences, and anything else is only listed.

use base64::{Engine, engine::general_purpose::STANDARD};
use corral_protocol::detail::{ArtifactEncoding, DetailBody, DetailRef, ReviewDetail};

use crate::model::assistant::sniff_image;
use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::id::Id;
use crate::model::nav::Form;

use super::view::{ArtifactContent, ArtifactView, ArtifactsView, Body, ButtonStyle, button};
use super::{Action, State};

pub(super) fn artifact_ref(task: &Id, key: &str) -> DetailRef {
    DetailRef::Artifact {
        task: task.to_string(),
        key: key.to_owned(),
    }
}

/// Removes ANSI escape sequences and other control characters except line breaks and tabs.
fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: parameters and intermediates up to a final byte in @..~.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: up to BEL or ST.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The client assembles every page into `body()`: text joined, base64 pages decoded, joined
/// and encoded once.
fn content(feed: &Feed, task: &Id, key: &str, media: Option<&str>, env: Env) -> ArtifactContent {
    let Some(cache) = feed.detail(&artifact_ref(task, key)) else {
        return ArtifactContent::Loading;
    };
    let Some(DetailBody::Artifact(page)) = cache.body() else {
        return match &cache.error {
            Some(error) => ArtifactContent::Error(format!(
                "corrald could not serve this artifact: {}",
                error.message
            )),
            None => ArtifactContent::Loading,
        };
    };
    let image = |media: &str| {
        if env.form == Form::Terminal {
            return ArtifactContent::Unsupported(
                "A terminal cannot draw images; open this candidate in the desktop or web app to see it.",
            );
        }
        ArtifactContent::Image {
            uri: format!("data:{media};base64,{}", page.content),
            truncated: page.truncated,
        }
    };
    match page.encoding {
        ArtifactEncoding::Text => ArtifactContent::Text {
            text: plain_text(&page.content),
            truncated: page.truncated,
        },
        ArtifactEncoding::Base64 => match page.media_type.as_deref().or(media) {
            Some(media) if media.starts_with("image/") => image(media),
            _ => {
                let Ok(bytes) = STANDARD.decode(&page.content) else {
                    return ArtifactContent::Unsupported(BINARY);
                };
                let bytes = bytes.as_slice();
                if let Some(media) = sniff_image(bytes) {
                    image(media)
                } else if let Some(text) = utf8_text(bytes, page.truncated) {
                    ArtifactContent::Text {
                        text: plain_text(text),
                        truncated: page.truncated,
                    }
                } else {
                    ArtifactContent::Unsupported(BINARY)
                }
            }
        },
        ArtifactEncoding::Other => ArtifactContent::Unsupported(BINARY),
    }
}

const BINARY: &str =
    "This artifact is binary and not an image, so the app lists it without showing its contents.";

/// The bytes as text when they are UTF-8 without NULs. A truncated page may end inside a
/// character; its complete prefix still counts.
fn utf8_text(bytes: &[u8], truncated: bool) -> Option<&str> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    (!text.contains('\0')).then_some(text)
}

/// The Artifacts tab: each record, opened ones with their contents.
pub(super) fn artifacts(
    state: &State,
    feed: &Feed,
    task: &Id,
    review: &ReviewDetail,
    env: Env,
) -> Body {
    Body::Artifacts(ArtifactsView {
        rows: review
            .artifacts
            .iter()
            .map(|artifact| {
                let key = artifact.source_ref.clone();
                let open = state.opened.contains(&(task.clone(), key.clone()));
                ArtifactView {
                    title: artifact.title.clone(),
                    source: key.clone(),
                    kind: artifact
                        .media_type
                        .clone()
                        .unwrap_or_else(|| "unknown type".into()),
                    content: if open {
                        content(feed, task, &key, artifact.media_type.as_deref(), env)
                    } else {
                        ArtifactContent::Closed
                    },
                    toggle: if open {
                        button("Hide", Action::CloseArtifact(key), ButtonStyle::Quiet)
                    } else {
                        button("Show", Action::OpenArtifact(key), ButtonStyle::Quiet)
                    },
                }
            })
            .collect(),
        empty: review
            .artifacts
            .is_empty()
            .then_some("No artifacts are recorded for this candidate."),
        note: "Contents come from corrald's artifact catalog in bounded pages, never from a file path on this device.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_control_sequences_never_reach_the_page() {
        assert_eq!(
            plain_text("\u{1b}[1mbold\u{1b}[0m\r\n\u{1b}]0;title\u{7}ok\u{8}"),
            "bold\nok"
        );
    }

    #[test]
    fn served_bytes_are_text_only_when_utf8_without_nul() {
        assert_eq!(utf8_text(b"summary\n", false), Some("summary\n"));
        assert_eq!(utf8_text("caf\u{e9}".as_bytes(), false), Some("caf\u{e9}"));
        assert_eq!(utf8_text(&"caf\u{e9}".as_bytes()[..4], true), Some("caf"));
        assert_eq!(utf8_text(&"caf\u{e9}".as_bytes()[..4], false), None);
        assert_eq!(utf8_text(b"a\0b", false), None);
        assert_eq!(utf8_text(&[0xff, 0xfe], false), None);
        assert_eq!(utf8_text(b"", false), Some(""));
    }
}

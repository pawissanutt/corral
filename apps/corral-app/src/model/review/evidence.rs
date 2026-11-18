//! The task's artifacts: catalog records, and on request their bounded contents from corrald's
//! artifact catalog (`DetailRef::Artifact` by reference), never a host file. corrald may serve
//! raw bytes without a media type, so the app tells them apart: an image by its own magic
//! bytes becomes a `data:` URI where the form can draw one, UTF-8 is shown as text without
//! terminal control sequences, and anything else is only listed.

use base64::{Engine, engine::general_purpose::STANDARD};
use corral_protocol::detail::{ArtifactEncoding, ArtifactPage, DetailBody};

use crate::model::assistant::sniff_image;
use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::id::Id;
use crate::model::nav::Form;

use super::live::{Shown, actor, artifact_ref};
use super::view::{ArtifactContent, ArtifactView, ArtifactsView, Body, ButtonStyle, button};
use super::{Action, State};

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

/// The assembled page of an artifact once it has arrived, or why it has not.
fn page<'a>(feed: &'a Feed, reference: &str) -> Result<&'a ArtifactPage, Option<String>> {
    let Some(cache) = feed.detail(&artifact_ref(reference)) else {
        return Err(None);
    };
    match cache.body() {
        Some(DetailBody::Artifact(page)) => Ok(page),
        _ => Err(cache.error.as_ref().map(|error| {
            format!("corrald could not serve this artifact: {}", error.message)
        })),
    }
}

/// An artifact's contents as text, for a log.
pub(super) enum TextPage {
    Loading,
    Failed(String),
    Text { text: String, truncated: bool },
}

pub(super) fn artifact_text(feed: &Feed, reference: &str) -> TextPage {
    let page = match page(feed, reference) {
        Ok(page) => page,
        Err(None) => return TextPage::Loading,
        Err(Some(why)) => return TextPage::Failed(why),
    };
    let text = match page.encoding {
        ArtifactEncoding::Text => Some(page.content.clone()),
        ArtifactEncoding::Base64 => STANDARD
            .decode(&page.content)
            .ok()
            .and_then(|bytes| utf8_text(&bytes, page.truncated).map(str::to_owned)),
        ArtifactEncoding::Other => None,
    };
    match text {
        Some(text) => TextPage::Text {
            text: plain_text(&text),
            truncated: page.truncated,
        },
        None => TextPage::Failed(BINARY.into()),
    }
}

/// The client assembles every page into `body()`: text joined, base64 pages decoded, joined
/// and encoded once.
fn content(feed: &Feed, reference: &str, env: Env) -> ArtifactContent {
    let page = match page(feed, reference) {
        Ok(page) => page,
        Err(None) => return ArtifactContent::Loading,
        Err(Some(why)) => return ArtifactContent::Error(why),
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
        ArtifactEncoding::Base64 => match page.media_type.as_deref() {
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

/// The Artifacts tab: the task's records from its detail, opened ones with their contents.
pub(super) fn artifacts(state: &State, feed: &Feed, shown: &Shown, env: Env) -> Body {
    let Some(detail) = shown.detail else {
        return Body::Notice("Loading the task's artifacts…".into());
    };
    let task: &Id = shown.id();
    Body::Artifacts(ArtifactsView {
        rows: detail
            .artifacts
            .iter()
            .map(|artifact| {
                let reference = artifact.reference();
                let open = state.opened.contains(&(task.clone(), reference.clone()));
                ArtifactView {
                    title: artifact.title.clone(),
                    kind: format!(
                        "{} · by {}",
                        artifact.kind.as_deref().unwrap_or("unknown kind"),
                        actor(artifact.by)
                    ),
                    content: if open {
                        content(feed, &reference, env)
                    } else {
                        ArtifactContent::Closed
                    },
                    toggle: if open {
                        button(
                            "Hide",
                            Action::CloseArtifact(reference.clone()),
                            ButtonStyle::Quiet,
                        )
                    } else {
                        button(
                            "Show",
                            Action::OpenArtifact(reference.clone()),
                            ButtonStyle::Quiet,
                        )
                    },
                    source: reference,
                }
            })
            .collect(),
        empty: detail
            .artifacts
            .is_empty()
            .then_some("No artifacts are recorded for this task."),
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

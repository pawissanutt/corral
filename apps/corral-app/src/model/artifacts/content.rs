//! What an artifact's served content is and how the finder shows it. Contents come from
//! corrald's artifact catalog (`DetailRef::Artifact` by reference), never from a host file.
//! corrald names a media type from the title and may serve raw bytes, so the app tells them
//! apart: Markdown, a diff, a log and other UTF-8 text are drawn (without terminal control
//! sequences); an image is drawn full size where the form can draw one; anything else is shown
//! as its facts.

use base64::{Engine, engine::general_purpose::STANDARD};
use corral_protocol::detail::{ArtifactEncoding, ArtifactPage, DetailBody, DetailRef};
use corral_types::{Artifact, ArtifactKind, section_slug};

use crate::model::assistant::sniff_image;
use crate::model::daemon::Feed;
use crate::model::{Form, Hue};

/// Lines the preview beside the results shows before it says how many more there are.
pub const PREVIEW_LINES: usize = 40;

const BINARY: &str = "Binary content: the app shows its facts, not its bytes.";
const HTML: &str = "An HTML page: the app shows its facts here rather than its markup. Open it full size to read the source.";
const IMAGE: &str = "An image: open it full size to see it.";
const TERMINAL_IMAGE: &str =
    "A terminal cannot draw images; open this artifact in the desktop or web app to see it.";
const CUT: &str =
    "corrald stops this artifact at its size bound; the rest is not served to the app.";

/// The request that reads an artifact's content.
pub fn target(reference: &str) -> DetailRef {
    DetailRef::Artifact {
        reference: reference.to_owned(),
    }
}

/// One line of a diff or a log, its colour decided here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub hue: Hue,
}

/// How the content is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Loading,
    Failed(String),
    Markdown(String),
    Diff(Vec<Line>),
    Log(Vec<Line>),
    Text(String),
    /// A `data:` URI the form can draw.
    Image(String),
    /// Not drawn: why, beside the artifact's facts.
    Facts(&'static str),
}

/// The content and, when not all of it shows, what is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub body: Body,
    pub cut: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Markdown,
    Diff,
    Log,
    Html,
    Text,
}

fn ends_with(title: &str, endings: &[&str]) -> bool {
    let title = title.to_ascii_lowercase();
    endings.iter().any(|ending| title.ends_with(ending))
}

/// What the text is, from corrald's media type, then the artifact's kind and title.
fn shape(artifact: &Artifact, media: Option<&str>) -> Shape {
    let title = artifact.title.as_str();
    match media.unwrap_or_default() {
        "text/markdown" => return Shape::Markdown,
        "text/x-diff" | "text/x-patch" => return Shape::Diff,
        "text/html" => return Shape::Html,
        _ => {}
    }
    let diff = matches!(artifact.kind, ArtifactKind::Diff);
    let log = matches!(artifact.kind, ArtifactKind::Gate | ArtifactKind::Log);
    if ends_with(title, &[".md", ".markdown"]) {
        Shape::Markdown
    } else if diff || ends_with(title, &[".diff", ".patch"]) {
        Shape::Diff
    } else if log || ends_with(title, &[".log"]) {
        Shape::Log
    } else if ends_with(title, &[".html", ".htm"]) {
        Shape::Html
    } else {
        Shape::Text
    }
}

/// A gate or run log line's colour: failures, then passes; anything else is ink.
pub(crate) fn log_hue(line: &str) -> Hue {
    let lower = line.to_ascii_lowercase();
    if lower.contains("fail") || lower.contains("error") {
        Hue::Fail
    } else if lower.contains("pass") {
        Hue::Ok
    } else {
        Hue::Neutral
    }
}

/// A unified diff line's colour: additions pass, deletions fail, file and hunk heads are quiet.
fn diff_hue(line: &str) -> Hue {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
        Hue::Neutral
    } else if line.starts_with('+') {
        Hue::Ok
    } else if line.starts_with('-') {
        Hue::Fail
    } else if line.starts_with("@@") {
        Hue::Corral
    } else {
        Hue::Neutral
    }
}

/// Removes ANSI escape sequences and other control characters except line breaks and tabs.
pub(crate) fn plain_text(text: &str) -> String {
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

/// The bytes as text when they are UTF-8 without NULs. A truncated page may end inside a
/// character; its complete prefix still counts.
pub(crate) fn utf8_text(bytes: &[u8], truncated: bool) -> Option<&str> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    (!text.contains('\0')).then_some(text)
}

/// What corrald served for the artifact, decoded: its text, else its image's media type.
enum Served {
    Text(String),
    Image(&'static str),
    Binary,
}

fn served(page: &ArtifactPage) -> Served {
    match page.encoding {
        ArtifactEncoding::Text => Served::Text(plain_text(&page.content)),
        ArtifactEncoding::Base64 => {
            let Ok(bytes) = STANDARD.decode(&page.content) else {
                return Served::Binary;
            };
            if let Some(media) = sniff_image(&bytes) {
                Served::Image(media)
            } else if let Some(text) = utf8_text(&bytes, page.truncated) {
                Served::Text(plain_text(text))
            } else {
                Served::Binary
            }
        }
        ArtifactEncoding::Other => Served::Binary,
    }
}

/// The first `limit` lines of `text`, and how many there are in all.
fn first_lines(text: &str, limit: Option<usize>) -> (String, Option<usize>) {
    let total = text.lines().count();
    match limit {
        Some(limit) if total > limit => (
            text.lines().take(limit).collect::<Vec<_>>().join("\n"),
            Some(total),
        ),
        _ => (text.to_owned(), None),
    }
}

/// The artifact's content as the finder draws it: in the preview (`full` false) at most
/// [`PREVIEW_LINES`] lines, with an HTML page and an image shown as facts; full size, all of it.
pub fn content(feed: &Feed, artifact: &Artifact, form: Form, full: bool) -> Content {
    let reference = artifact.reference();
    let Some(cache) = feed.detail(&target(&reference)) else {
        return Content {
            body: Body::Loading,
            cut: None,
        };
    };
    let page = match cache.body() {
        Some(DetailBody::Artifact(page)) => page,
        _ => {
            let body = match &cache.error {
                Some(error) => Body::Failed(format!(
                    "corrald could not serve {reference}: {}",
                    error.message
                )),
                None => Body::Loading,
            };
            return Content { body, cut: None };
        }
    };
    let bounded = page.truncated.then(|| CUT.to_owned());
    let text = match served(page) {
        Served::Text(text) => text,
        Served::Image(media) => {
            let body = match (full, form) {
                (_, Form::Terminal) => Body::Facts(TERMINAL_IMAGE),
                (false, _) => Body::Facts(IMAGE),
                (true, _) => Body::Image(format!("data:{media};base64,{}", page.content)),
            };
            return Content { body, cut: bounded };
        }
        Served::Binary => {
            return Content {
                body: Body::Facts(BINARY),
                cut: None,
            };
        }
    };
    let shape = shape(artifact, page.media_type.as_deref());
    if shape == Shape::Html && !full {
        return Content {
            body: Body::Facts(HTML),
            cut: None,
        };
    }
    let (text, total) = first_lines(&text, (!full).then_some(PREVIEW_LINES));
    let lines = |hue: fn(&str) -> Hue| {
        text.lines()
            .map(|line| Line {
                text: line.to_owned(),
                hue: hue(line),
            })
            .collect()
    };
    let body = match shape {
        Shape::Markdown => Body::Markdown(text.clone()),
        Shape::Diff => Body::Diff(lines(diff_hue)),
        Shape::Log => Body::Log(lines(log_hue)),
        Shape::Html | Shape::Text => Body::Text(text.clone()),
    };
    let cut = total
        .map(|total| {
            format!("The first {PREVIEW_LINES} of {total} lines; open it full size for the rest.")
        })
        .or(bounded);
    Content { body, cut }
}

/// The Markdown section whose heading's slug is `slug`: its heading and everything up to the
/// next heading of its level or above, fenced code skipped. Slugs are corral-types'.
fn section<'a>(text: &'a str, slug: &str) -> Option<&'a str> {
    let mut fence: Option<&str> = None;
    let mut found: Option<(usize, usize)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            continue;
        }
        let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        let words = &trimmed[level..];
        if !(1..=6).contains(&level) || !(words.trim().is_empty() || words.starts_with([' ', '\t']))
        {
            continue;
        }
        match found {
            Some((begin, open)) if level <= open => return Some(&text[begin..start]),
            Some(_) => {}
            None => {
                let words = words.trim().trim_end_matches('#').trim_end();
                if section_slug(words) == slug {
                    found = Some((start, level));
                }
            }
        }
    }
    found.map(|(begin, _)| &text[begin..])
}

/// `content` cut to the cited section when it is Markdown that has it; otherwise all of it,
/// saying why.
pub(crate) fn at_section(content: Content, slug: &str) -> Content {
    match content.body {
        Body::Markdown(text) => match section(&text, slug) {
            Some(part) => Content {
                body: Body::Markdown(part.to_owned()),
                cut: Some(format!(
                    "Only the cited section “{slug}” shows; open the artifact from the results for all of it."
                )),
            },
            None => Content {
                body: Body::Markdown(text),
                cut: Some(format!(
                    "No section “{slug}” in this artifact; all of it shows."
                )),
            },
        },
        Body::Loading | Body::Failed(_) => content,
        body => Content {
            body,
            cut: Some(format!(
                "Sections are only found in Markdown; all of this artifact shows, not “{slug}”."
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cited section must end at the next heading of its level, not at a deeper one or at a
    /// heading-looking line inside fenced code.
    #[test]
    fn a_cited_section_runs_to_the_next_heading_of_its_level() {
        let text =
            "# Doc\n## Goal\nwhy\n### Detail\n```\n## not a heading\n```\n## Acceptance\nok\n";
        assert_eq!(
            section(text, "goal"),
            Some("## Goal\nwhy\n### Detail\n```\n## not a heading\n```\n")
        );
        assert_eq!(section(text, "acceptance"), Some("## Acceptance\nok\n"));
        assert_eq!(section(text, "not-a-heading"), None);
    }

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

//! What an artifact's served content is and how the finder shows it. Contents come from
//! corrald's artifact catalog (`DetailRef::Artifact` by reference), never from a host file.
//! corrald names a media type from the title and may serve raw bytes, so the app tells them
//! apart: Markdown, a diff, a log and other UTF-8 text are drawn (without terminal control
//! sequences); an image is drawn full size where the form can draw one; anything else is shown
//! as its facts.

use base64::{Engine, engine::general_purpose::STANDARD};
use corral_protocol::detail::{ArtifactEncoding, ArtifactPage, DetailBody, DetailRef};
use corral_types::{Artifact, ArtifactKind};

use crate::model::assistant::sniff_image;
use crate::model::daemon::Feed;
use crate::model::{Form, Hue};

/// Lines the preview beside the results shows before it says how many more there are.
pub const PREVIEW_LINES: usize = 40;
/// Bytes of text the preview shows at most, so one very long line stays bounded too.
pub const PREVIEW_BYTES: usize = 4 * 1024;
/// Lines a full-size view of a partial head draws at most; a complete artifact draws whole.
pub const FULL_PARTIAL_LINES: usize = 1000;

const BINARY: &str =
    "Binary content: the app shows its facts, not its bytes. Download saves the bytes.";
const HTML: &str = "An HTML page: the app shows its facts here rather than its markup. Open in browser shows it in a sandboxed copy; View source reads the markup.";
/// Bytes of a page's start searched for an HTML signature.
const SIGNATURE_BYTES: usize = 1024;
/// Bytes of a text's start searched for Markdown structure.
const MARKDOWN_SIGNATURE_BYTES: usize = 64 * 1024;
const IMAGE: &str = "An image: open it full size to see it.";
const TERMINAL_IMAGE: &str =
    "A terminal cannot draw images; open this artifact in the desktop or web app to see it.";
const PARTIAL_IMAGE: &str =
    "An image too large to preview here: the app draws only a whole picture. Download saves it.";

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

/// Whether the text starts as a standalone HTML document does (`<!doctype html`, `<html`),
/// after a byte-order mark, white space and comments, within its first [`SIGNATURE_BYTES`]:
/// a descriptive title names no suffix, and corrald's media type follows the title, so this
/// start is the only evidence that opens a browser.
pub(crate) fn html_signature(text: &str) -> bool {
    let mut start = text[..text.floor_char_boundary(SIGNATURE_BYTES)]
        .trim_start_matches('\u{feff}')
        .trim_start();
    while let Some(comment) = start.strip_prefix("<!--") {
        let Some(end) = comment.find("-->") else {
            return false;
        };
        start = comment[end + 3..].trim_start();
    }
    let opens = |tag: &str| {
        start
            .get(..tag.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(tag))
            && start[tag.len()..]
                .chars()
                .next()
                .is_some_and(|c| c == '>' || c.is_whitespace())
    };
    opens("<!doctype html") || opens("<html")
}

/// What the text is, from corrald's media type, an HTML signature, then the artifact's kind
/// and title, and last the text's own Markdown structure.
fn shape(artifact: &Artifact, media: Option<&str>, page: &ArtifactPage) -> Shape {
    // corrald names `text/html` from the title alone, so neither it nor a `.html` suffix makes
    // a page: only the served bytes' own start does.
    if original_text(page).is_some_and(|text| html_signature(&text)) {
        return Shape::Html;
    }
    let title = artifact.title.as_str();
    match media.unwrap_or_default() {
        "text/markdown" => return Shape::Markdown,
        "text/x-diff" | "text/x-patch" => return Shape::Diff,
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
    } else if !file_name(title) && original_text(page).is_some_and(|text| markdown_signature(&text))
    {
        Shape::Markdown
    } else {
        Shape::Text
    }
}

/// Whether the title is a file's name with an extension (`deploy.sh`, `notes.txt`): such a file
/// says what it is, and its `#` comments or fences are not Markdown. A descriptive title
/// (`Delivery plan`, `Plan v1.2`) names none.
fn file_name(title: &str) -> bool {
    title.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && !stem.ends_with(char::is_whitespace)
            && (1..=10).contains(&extension.len())
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && extension.bytes().any(|byte| byte.is_ascii_alphabetic())
    })
}

/// Whether the text reads as a Markdown document by its own structure: its first line that says
/// something is an ATX heading (`# Plan`), or, within its first 64 KiB, it holds a closed code fence (```` ``` ```` or
/// `~~~`, a Mermaid diagram's included) or a table's delimiter row (`|---|---|`). A descriptive
/// title (`Delivery plan`) names no suffix and corrald's media type follows the title, so this
/// is what renders a titled agent document as Markdown rather than its source.
pub(crate) fn markdown_signature(text: &str) -> bool {
    let text = &text[..text.floor_char_boundary(MARKDOWN_SIGNATURE_BYTES)];
    let mut lines = text.lines().map(str::trim_end);
    let heading = |line: &str| {
        let marks = line.bytes().take_while(|byte| *byte == b'#').count();
        (1..=6).contains(&marks) && line[marks..].starts_with(' ') && line.len() > marks + 1
    };
    if lines
        .clone()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| heading(line.trim_start_matches('\u{feff}')))
    {
        return true;
    }
    let mut fence: Option<&str> = None;
    for line in lines.by_ref() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        match fence {
            Some(open)
                if indent <= 3
                    && trimmed.len() >= open.len()
                    && trimmed.bytes().all(|byte| byte == open.as_bytes()[0]) =>
            {
                return true;
            }
            Some(_) => {}
            None if indent <= 3 && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) => {
                let mark = trimmed.as_bytes()[0];
                let run = trimmed.bytes().take_while(|byte| *byte == mark).count();
                fence = Some(&trimmed[..run]);
            }
            None if table_delimiter(trimmed) => return true,
            None => {}
        }
    }
    false
}

/// A GitHub table's delimiter row: cells of dashes, optionally aligned with colons, between
/// pipes (`|---|:--:|`).
fn table_delimiter(line: &str) -> bool {
    let line = line.trim();
    let Some(inner) = line.strip_prefix('|') else {
        return false;
    };
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    !inner.is_empty()
        && inner.split('|').all(|cell| {
            let cell = cell.trim().trim_start_matches(':').trim_end_matches(':');
            !cell.is_empty() && cell.bytes().all(|byte| byte == b'-')
        })
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

/// The served head's text as corrald sent it, before control characters are removed; none for
/// bytes that are not UTF-8.
fn original_text(page: &ArtifactPage) -> Option<std::borrow::Cow<'_, str>> {
    match page.encoding {
        ArtifactEncoding::Text => Some(page.content.as_str().into()),
        ArtifactEncoding::Base64 => {
            let bytes = STANDARD.decode(&page.content).ok()?;
            utf8_text(&bytes, page.truncated).map(|text| text.to_owned().into())
        }
        ArtifactEncoding::Other => None,
    }
}

/// Whether the artifact corrald serves is a standalone HTML page, by the signature its own
/// served head starts with; false until its content has loaded and for a cited section.
pub fn is_html(feed: &Feed, artifact: &Artifact) -> bool {
    let Some(DetailBody::Artifact(page)) = feed
        .detail(&target(&artifact.reference()))
        .and_then(|cache| cache.body())
    else {
        return false;
    };
    match served(page) {
        Served::Text(_) => shape(artifact, page.media_type.as_deref(), page) == Shape::Html,
        Served::Image(_) | Served::Binary => false,
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
    content_of(feed, artifact, &artifact.reference(), form, full)
}

/// [`content`] as corrald serves it under `reference`: the artifact's, or for a cited section
/// (`T-12:3#goal`) the section the engine cuts.
pub fn content_of(
    feed: &Feed,
    artifact: &Artifact,
    reference: &str,
    form: Form,
    full: bool,
) -> Content {
    let Some(cache) = feed.detail(&target(reference)) else {
        return Content {
            body: Body::Loading,
            cut: None,
        };
    };
    let page = match cache.body() {
        Some(DetailBody::Artifact(page)) => page,
        _ => {
            let body = match &cache.error {
                // The device's bound, not corrald's: what is cited exists and is too large to
                // read here.
                Some(error) if error.code == "too_large" => Body::Failed(format!(
                    "{reference} is larger than this device previews ({}). Download saves all of it; `corral artifact show {reference}` shows it too.",
                    error.message.trim_end_matches('.')
                )),
                Some(error) => Body::Failed(format!(
                    "corrald could not serve {reference}: {}",
                    error.message
                )),
                None => Body::Loading,
            };
            return Content { body, cut: None };
        }
    };
    // What the whole is, when the reference names the whole artifact rather than a section.
    let total = (!reference.contains('#')).then_some(artifact.bytes);
    let text = match served(page) {
        Served::Text(text) => text,
        // An image is drawn only whole: incomplete bytes would draw a broken picture.
        Served::Image(_) if page.truncated => {
            return Content {
                body: Body::Facts(PARTIAL_IMAGE),
                cut: None,
            };
        }
        Served::Image(media) => {
            let body = match (full, form) {
                (_, Form::Terminal) => Body::Facts(TERMINAL_IMAGE),
                (false, _) => Body::Facts(IMAGE),
                (true, _) => Body::Image(format!("data:{media};base64,{}", page.content)),
            };
            return Content { body, cut: None };
        }
        Served::Binary => {
            return Content {
                body: Body::Facts(BINARY),
                cut: None,
            };
        }
    };
    let shape = shape(artifact, page.media_type.as_deref(), page);
    if shape == Shape::Html && !full {
        return Content {
            body: Body::Facts(HTML),
            cut: None,
        };
    }
    let held = text.len();
    // Full size draws a complete artifact whole; a partial head stays within a line budget too.
    let limit = match (full, page.truncated) {
        (false, _) => Some((PREVIEW_LINES, PREVIEW_BYTES)),
        (true, true) => Some((FULL_PARTIAL_LINES, usize::MAX)),
        (true, false) => None,
    };
    let (text, lines_cut) = head(&text, limit);
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
    let cut = cut_note(&lines_cut, text.len(), held, page.truncated, total, full);
    Content { body, cut }
}

/// How a shown head was cut from what the app holds: by lines, or by bytes (one long line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    Whole,
    Lines { shown: usize, total: usize },
    Bytes { shown: usize },
}

/// The text the view draws: without a limit all that is held, else at most its lines and bytes,
/// cut at a character's boundary.
fn head(text: &str, limit: Option<(usize, usize)>) -> (String, Cut) {
    let Some((lines, bytes)) = limit else {
        return (text.to_owned(), Cut::Whole);
    };
    let (lined, total) = first_lines(text, Some(lines));
    if lined.len() > bytes {
        let end = lined.floor_char_boundary(bytes);
        return (lined[..end].to_owned(), Cut::Bytes { shown: end });
    }
    match total {
        Some(total) => (
            lined,
            Cut::Lines {
                shown: lines,
                total,
            },
        ),
        None => (lined, Cut::Whole),
    }
}

/// What the note under the content says is left out: the lines or bytes the preview leaves to
/// full size, and whether corrald holds more than the app read (a partial preview, with the size
/// of the whole when the reference names the whole artifact).
fn cut_note(
    cut: &Cut,
    drawn: usize,
    held: usize,
    partial: bool,
    total: Option<u64>,
    full: bool,
) -> Option<String> {
    let size = |bytes: u64| crate::model::session::transcript::size(bytes);
    if partial {
        let of = total
            .map(|total| format!(" of {}", size(total)))
            .unwrap_or_default();
        let lines = match cut {
            Cut::Lines { shown, .. } => format!("{shown} lines, "),
            Cut::Bytes { .. } | Cut::Whole => String::new(),
        };
        let more = if full {
            ""
        } else {
            " Open it full size for more of the head;"
        };
        return Some(format!(
            "Partial preview: the first {lines}{}{of}.{more} Download saves the whole artifact.",
            size(drawn as u64)
        ));
    }
    match cut {
        Cut::Whole => None,
        Cut::Lines { shown, total } => Some(format!(
            "The first {shown} of {total} lines; open it full size for the rest."
        )),
        Cut::Bytes { shown } => Some(format!(
            "The first {} of {}; open it full size for the rest.",
            size(*shown as u64),
            size(held as u64)
        )),
    }
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

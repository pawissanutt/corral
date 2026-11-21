//! Attachments on the next message: what the owner staged, how each item shows in the
//! composer's tray, the refs one send carries, and the facts a served message shows with their
//! canonical readback. The typed text stays the only owner input. Nothing here fetches a link
//! or holds a host path; bytes stay with the client, and readback comes from corrald.

use base64::{Engine, engine::general_purpose::STANDARD};
use corral_protocol::assistant::SendAttachment;
use corral_protocol::attachment::{
    ASSISTANT_ATTACH_V1, ATTACHMENT_IMAGE_MAX, ATTACHMENT_NAME_MAX, ATTACHMENT_TEXT_MAX,
    ATTACHMENT_URL_MAX, ATTACHMENTS_PER_MESSAGE, AttachmentFact, AttachmentKind,
};
use corral_protocol::detail::{ArtifactEncoding, DetailBody, DetailGet, DetailRef};

use super::views::{Button, Chip, button};
use super::{Action, State};
use crate::model::daemon::{ClientError, Feed, Link, UploadStatus};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::text::{Rich, span};

/// What the owner chose to attach from this device: a saved image or a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickKind {
    /// An image: pasted, or a path File names by its extension.
    Image,
    /// A UTF-8 text file, or an image File picked (File).
    File,
}

impl PickKind {
    /// The wire kind the upload declares.
    pub fn kind(self) -> AttachmentKind {
        match self {
            PickKind::Image => AttachmentKind::Image,
            PickKind::File => AttachmentKind::File,
        }
    }
}

/// The inline field the composer foot shows instead of its attachment buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachField {
    /// A web link, sent inline and never fetched.
    Link,
    /// A path on this device (terminal only): the client reads it off the UI thread.
    Path(PickKind),
}

impl AttachField {
    /// The field's empty-text prompt.
    pub const fn placeholder(self) -> &'static str {
        match self {
            AttachField::Link => "Paste a link (https://…)",
            AttachField::Path(PickKind::Image) => "Path to image…",
            AttachField::Path(PickKind::File) => "Path to file…",
        }
    }

    /// The word before the field in a terminal.
    pub const fn lead(self) -> &'static str {
        match self {
            AttachField::Link => "link",
            AttachField::Path(_) => "path",
        }
    }
}

/// One item staged for the next message. App-owned and in memory only: the bytes stay with the
/// client, and a path the owner typed never reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staged {
    /// An upload by its client identity; progress and state are the client's (`feed.uploads`).
    Upload {
        upload: Id,
        kind: AttachmentKind,
        name: String,
    },
    /// An http(s) link exactly as typed, sent inline; neither the app nor corrald fetches it.
    Link { url: String },
}

impl Staged {
    /// The key the tray, removal and a send's refs name it by.
    pub fn key(&self) -> &str {
        match self {
            Staged::Upload { upload, .. } => upload.as_str(),
            Staged::Link { url } => url,
        }
    }
}

/// A staged item as the tray draws it, in the context chip's anatomy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedChip {
    /// The staged key: upload id or link.
    pub key: String,
    /// "image", "file" or "link".
    pub kind: &'static str,
    /// File name, or the link's host and path.
    pub label: String,
    /// Size, or empty for a link.
    pub meta: String,
    /// Uploading, waiting or failed; absent once ready.
    pub state: Option<Chip>,
    /// Removes it from the next message.
    pub remove: Action,
    /// "Remove shot.png".
    pub remove_label: String,
}

/// The composer's attachment parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tray {
    /// File, enabled or not.
    pub buttons: Vec<Button>,
    /// What is staged.
    pub chips: Vec<StagedChip>,
    /// Why a button is dimmed.
    pub note: Option<String>,
    /// The line above the composer while attachments hold Send.
    pub status: Option<Rich>,
    /// Send waits: an item is not ready, or the recipient takes none.
    pub holds_send: bool,
}

/// Where one staged item stands.
enum Readiness {
    Ready,
    Waiting,
    Failed(String),
}

/// `1.2 MB`, `42 KB`, `512 B`.
pub fn byte_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    match bytes {
        b if b < KB => format!("{b} B"),
        b if b < KB * KB => format!("{} KB", b.div_ceil(KB)),
        b => format!("{:.1} MB", b as f64 / (KB * KB) as f64),
    }
}

/// A link as a chip names it: host and path without the scheme, at most 60 characters.
pub fn short_link(url: &str) -> String {
    let bare = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/');
    if bare.chars().count() <= 60 {
        return bare.to_owned();
    }
    let mut short: String = bare.chars().take(59).collect();
    short.push('…');
    short
}

/// What a client or corrald refusal means to the owner.
pub fn refusal_words(error: &ClientError) -> String {
    let words = match error.code.as_str() {
        "local_not_regular_file" => "not a regular file",
        "local_too_large" => "too large (images up to 2 MiB, text files up to 64 KiB)",
        "local_type_unsupported" => "not a PNG, JPEG, GIF or WebP image, nor UTF-8 text",
        "local_read_failed" => "this device could not read it",
        "attachment_type_unsupported" => "corrald does not accept this type",
        "attachment_too_large" => "over corrald's size limit",
        "attachment_quota" => "upload quota reached (8 waiting, or 64 MiB today)",
        "attachment_expired" => "the upload expired; attach it again",
        "digest_mismatch" => "the upload did not arrive intact",
        "offline" | "disconnected" => "corrald was not reachable",
        _ if !error.message.is_empty() => return error.message.clone(),
        code => return code.replace('_', " "),
    };
    words.to_owned()
}

fn readiness(item: &Staged, feed: &Feed) -> Readiness {
    let Staged::Upload { upload, .. } = item else {
        return Readiness::Ready;
    };
    match feed.uploads.get(upload).map(|state| &state.status) {
        Some(UploadStatus::Ready) => Readiness::Ready,
        Some(UploadStatus::Failed(error)) => Readiness::Failed(refusal_words(error)),
        Some(UploadStatus::Uploading) | None => Readiness::Waiting,
    }
}

fn kind_word(kind: &AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image",
        AttachmentKind::Link => "link",
        AttachmentKind::File | AttachmentKind::Other => "file",
    }
}

fn staged_chip(item: &Staged, feed: &Feed) -> StagedChip {
    let (kind, label, meta, state) = match item {
        Staged::Link { url } => ("link", short_link(url), String::new(), None),
        Staged::Upload { upload, kind, name } => {
            let served = feed.uploads.get(upload);
            let state = match served.map(|state| (&state.status, state.sent, state.bytes)) {
                Some((UploadStatus::Ready, ..)) => None,
                Some((UploadStatus::Failed(error), ..)) => Some(Chip::new(
                    format!("failed · {}", refusal_words(error)),
                    Hue::Fail,
                )),
                _ if feed.link != Link::Up => Some(Chip::new("waiting for corrald", Hue::Held)),
                Some((UploadStatus::Uploading, sent, total)) if total > 0 => Some(Chip::new(
                    format!("uploading {}%", (sent.min(total) * 100) / total),
                    Hue::Held,
                )),
                _ => Some(Chip::new("preparing…", Hue::Held)),
            };
            (
                kind_word(kind),
                served.map_or_else(|| name.clone(), |state| state.name.clone()),
                served.map_or_else(String::new, |state| byte_size(state.bytes)),
                state,
            )
        }
    };
    StagedChip {
        key: item.key().to_owned(),
        remove_label: format!("Remove {label}"),
        kind,
        label,
        meta,
        state,
        remove: Action::Unattach(item.key().to_owned()),
    }
}

/// The tray, its one File button (an image or a text file) with why it is dimmed, and what
/// holds Send. A file uploads, so it needs corrald now; images and files pasted into the field
/// stage through the same upload.
pub fn tray(feed: &Feed, staged: &[Staged], text_blank: bool) -> Tray {
    let capable = feed.capable(ASSISTANT_ATTACH_V1);
    let full = staged.len() >= ATTACHMENTS_PER_MESSAGE;
    let up = feed.link == Link::Up;
    let mut file = button("File", Action::Attach(PickKind::File));
    file.enabled = capable && !full && up;
    let note = if !capable {
        Some(if feed.capabilities.is_empty() {
            "attachments wait for a connection to corrald".to_owned()
        } else {
            "this corrald does not accept attachments; upgrade corral".to_owned()
        })
    } else if full {
        Some(format!("{ATTACHMENTS_PER_MESSAGE} attachments per message"))
    } else if !up {
        Some("files upload while corrald is reachable".into())
    } else {
        None
    };
    let states: Vec<(&Staged, Readiness)> = staged
        .iter()
        .map(|item| (item, readiness(item, feed)))
        .collect();
    let failed = states.iter().find_map(|(item, state)| match state {
        Readiness::Failed(why) => Some((staged_chip(item, feed).label, why.clone())),
        _ => None,
    });
    let waiting = states
        .iter()
        .any(|(_, state)| matches!(state, Readiness::Waiting));
    let status = if let Some((name, why)) = failed {
        Some(vec![
            span("attachment not uploaded").hue(Hue::Fail),
            span(format!(" {name}: {why}. Remove it to send.")),
        ])
    } else if waiting {
        Some(vec![
            span("uploading attachments").hue(Hue::Held),
            span(if up {
                " Send waits until every one is ready."
            } else {
                " They upload when corrald is reachable; Send waits for them."
            }),
        ])
    } else if !staged.is_empty() && text_blank {
        Some(vec![
            span("add a few words").hue(Hue::Held),
            span(" Attachments go with your message; they are never sent alone."),
        ])
    } else {
        None
    };
    Tray {
        buttons: vec![file],
        chips: staged.iter().map(|item| staged_chip(item, feed)).collect(),
        note,
        status,
        holds_send: states.iter().any(|(_, s)| !matches!(s, Readiness::Ready)),
    }
}

/// The refs one send carries, in staging order: every upload must be ready.
pub fn send_refs(staged: &[Staged], feed: &Feed) -> Result<Vec<SendAttachment>, String> {
    if staged.len() > ATTACHMENTS_PER_MESSAGE {
        return Err(format!(
            "Not sent: {ATTACHMENTS_PER_MESSAGE} attachments per message."
        ));
    }
    staged
        .iter()
        .map(|item| match item {
            Staged::Link { url } => Ok(SendAttachment::Link {
                url: url.clone(),
                title: None,
            }),
            Staged::Upload { upload, name, .. } => match feed.uploads.get(upload) {
                Some(state) if state.status == UploadStatus::Ready => Ok(SendAttachment::Upload {
                    upload: upload.to_string(),
                    sha256: state.sha256.clone(),
                }),
                _ => Err(format!("Not sent: {name} is not uploaded yet.")),
            },
        })
        .collect()
}

/// A typed link, trimmed and otherwise kept as typed: http(s) only, no user name or password,
/// no spaces or control characters, within the wire bound. It is never fetched.
pub fn check_link(text: &str) -> Result<String, String> {
    let url = text.trim();
    if url.is_empty() {
        return Err("paste a link first".into());
    }
    if url.len() > ATTACHMENT_URL_MAX {
        return Err(format!("longer than {ATTACHMENT_URL_MAX} bytes"));
    }
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else {
        return Err("only http:// and https:// links attach".into());
    };
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("a link has no spaces or control characters".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err("the link names no host".into());
    }
    if authority.contains('@') {
        return Err("links with a user name or password are not attached".into());
    }
    Ok(url.to_owned())
}

/// A file name the wire takes: control characters dropped, at most the wire's bytes.
pub fn bounded_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len().min(ATTACHMENT_NAME_MAX));
    for c in name.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > ATTACHMENT_NAME_MAX {
            break;
        }
        out.push(c);
    }
    let out = out.trim();
    if out.is_empty() {
        "attachment".into()
    } else {
        out.to_owned()
    }
}

/// The image format corrald accepts that the bytes start with.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some("image/png"),
        [0xff, 0xd8, 0xff, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

/// A picked file as the upload declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub kind: AttachmentKind,
    pub name: String,
    pub media_type: String,
}

/// Checks a picked file against what corrald accepts before anything is uploaded: an image is a
/// PNG, JPEG, GIF or WebP of at most 2 MiB (an image picked as a file counts as one); a file is
/// UTF-8 text without NUL of at most 64 KiB. The refusal names the file.
pub fn classify(
    pick: PickKind,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<Classified, String> {
    let name = bounded_name(name);
    let len = bytes.len() as u64;
    if len == 0 {
        return Err(format!("{name} is empty; not attached"));
    }
    if let Some(media) = sniff_image(bytes) {
        if len > ATTACHMENT_IMAGE_MAX {
            return Err(format!("{name} is over 2 MiB; not attached"));
        }
        return Ok(Classified {
            kind: AttachmentKind::Image,
            name,
            media_type: media.into(),
        });
    }
    if pick == PickKind::Image {
        return Err(format!(
            "{name} is not a PNG, JPEG, GIF or WebP image; not attached"
        ));
    }
    if len > ATTACHMENT_TEXT_MAX {
        return Err(format!(
            "{name} is over 64 KiB; text files up to 64 KiB attach"
        ));
    }
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return Err(format!(
            "{name} is not UTF-8 text; PDFs and other binaries do not attach"
        ));
    }
    let media = mime.trim();
    Ok(Classified {
        kind: AttachmentKind::File,
        name,
        media_type: if media.starts_with("text/") {
            media.into()
        } else {
            "text/plain".into()
        },
    })
}

/// An attachment a message carries, as its chip in the message header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChip {
    /// `image shot.png · 1.2 MB`, `link nsh.dev/elements/…`.
    pub label: String,
    /// Opens its canonical readback under the message; a link is never opened by the app.
    pub open: Option<Action>,
    /// Its readback is open.
    pub opened: bool,
}

/// What corrald served of an opened attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readback {
    Loading,
    Failed(String),
    Unsupported(&'static str),
    /// UTF-8 text without control characters other than line breaks and tabs.
    Text {
        text: String,
        truncated: bool,
    },
    /// A `data:` URI of a sniffed PNG, JPEG, GIF or WebP.
    Image {
        uri: String,
        truncated: bool,
    },
}

/// An opened attachment under its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentView {
    pub title: String,
    pub body: Readback,
    pub close: Action,
}

/// The canonical readback of attachment `id` of message `entry`.
pub fn attachment_ref(entry: &str, id: &str) -> DetailRef {
    DetailRef::Attachment {
        entry: entry.to_owned(),
        id: id.to_owned(),
    }
}

fn fact_label(fact: &AttachmentFact) -> String {
    let name = bounded_name(&fact.name);
    match fact.kind {
        AttachmentKind::Link => format!("link {}", fact.url.as_deref().map_or(name, short_link)),
        ref kind => match fact.bytes {
            Some(bytes) => format!("{} {name} · {}", kind_word(kind), byte_size(bytes)),
            None => format!("{} {name}", kind_word(kind)),
        },
    }
}

/// The chips of a served message's attachments, and the open one's readback.
pub fn fact_chips(
    state: &State,
    feed: &Feed,
    entry: &str,
    facts: &[AttachmentFact],
) -> (Vec<FileChip>, Option<AttachmentView>) {
    let open = state.live.attachment.as_ref();
    let mut view = None;
    let chips = facts
        .iter()
        .map(|fact| {
            let readable = matches!(fact.kind, AttachmentKind::Image | AttachmentKind::File);
            let opened = readable && open.is_some_and(|(at, id)| at == entry && *id == fact.id);
            if opened {
                view = Some(AttachmentView {
                    title: bounded_name(&fact.name),
                    body: readback(feed, entry, fact),
                    close: Action::OpenAttachment {
                        entry: entry.to_owned(),
                        id: fact.id.clone(),
                    },
                });
            }
            FileChip {
                label: fact_label(fact),
                open: readable.then(|| Action::OpenAttachment {
                    entry: entry.to_owned(),
                    id: fact.id.clone(),
                }),
                opened,
            }
        })
        .collect();
    (chips, view)
}

/// The chips of transcript message `entry` (its served id) and the open one's readback, read
/// from whichever session page this device holds it in; none while it carries nothing.
pub fn message_attachments(
    state: &State,
    feed: &Feed,
    entry: &str,
) -> (Vec<FileChip>, Option<AttachmentView>) {
    let facts = feed
        .details
        .values()
        .filter_map(|cache| match cache.body() {
            Some(DetailBody::Session(page)) => Some(page),
            _ => None,
        })
        .flat_map(|page| page.messages.iter())
        .find(|message| !message.id.is_empty() && message.id == entry)
        .map(|message| message.attachments.as_slice())
        .unwrap_or_default();
    fact_chips(state, feed, entry, facts)
}

/// Removes escape sequences' introducer and every control character except line breaks and
/// tabs, so a served text file cannot drive a terminal.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}

fn readback(feed: &Feed, entry: &str, fact: &AttachmentFact) -> Readback {
    artifact_readback(feed, &attachment_ref(entry, &fact.id), &fact.kind)
}

/// What corrald served at `target` (an attachment or any artifact) read back as `kind`: safe
/// text, a sniffed image, or why it is not shown. Never fetches; the caller loads `target`.
pub fn artifact_readback(feed: &Feed, target: &DetailRef, kind: &AttachmentKind) -> Readback {
    let Some(cache) = feed.detail(target) else {
        return Readback::Loading;
    };
    let Some(DetailBody::Artifact(page)) = cache.body() else {
        return match &cache.error {
            Some(error) => Readback::Failed(format!(
                "corrald could not serve this {}: {}",
                kind_word(kind),
                refusal_words(error)
            )),
            None => Readback::Loading,
        };
    };
    match (kind, &page.encoding) {
        (AttachmentKind::File, ArtifactEncoding::Text) => Readback::Text {
            text: plain(&page.content),
            truncated: page.truncated,
        },
        (AttachmentKind::File, ArtifactEncoding::Base64) => match STANDARD
            .decode(&page.content)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            Some(text) => Readback::Text {
                text: plain(&text),
                truncated: page.truncated,
            },
            None => Readback::Unsupported(
                "corrald served bytes that are not UTF-8 text; they are not shown.",
            ),
        },
        (AttachmentKind::Image, ArtifactEncoding::Base64) => {
            // The first 12 bytes are 16 base64 characters: enough to sniff the format.
            let head = page
                .content
                .get(..16)
                .and_then(|head| STANDARD.decode(head).ok());
            match head.as_deref().and_then(sniff_image) {
                Some(media) => Readback::Image {
                    uri: format!("data:{media};base64,{}", page.content),
                    truncated: page.truncated,
                },
                None => Readback::Unsupported(
                    "corrald served bytes that are not a PNG, JPEG, GIF or WebP image; they are not shown.",
                ),
            }
        }
        _ => Readback::Unsupported("This attachment is not text or an image the app shows."),
    }
}

impl State {
    /// Opens or closes an attachment's readback under its message, loading it once.
    pub fn open_attachment(&mut self, entry: String, id: String, feed: &Feed) -> Vec<Effect> {
        let pair = (entry, id);
        if self.live.attachment.as_ref() == Some(&pair) {
            self.live.attachment = None;
            return Vec::new();
        }
        let target = attachment_ref(&pair.0, &pair.1);
        self.live.attachment = Some(pair);
        let fresh = feed.detail(&target).is_some_and(|cache| {
            cache.loading || (!cache.stale && cache.error.is_none() && cache.body().is_some())
        });
        if fresh {
            return Vec::new();
        }
        vec![Effect::Detail(DetailGet {
            target,
            etag: None,
            page: None,
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_kept_as_typed_and_only_web_links_without_userinfo_pass() {
        assert_eq!(
            check_link("  https://nsh.dev/elements/diff-view?x=1#y ").as_deref(),
            Ok("https://nsh.dev/elements/diff-view?x=1#y")
        );
        assert!(check_link("HTTP://example.com").is_ok());
        for refused in [
            "",
            "file:///etc/passwd",
            "data:text/plain,hi",
            "javascript:alert(1)",
            "https://user:pw@example.com/",
            "https://exa mple.com",
            "https://",
            "https://a.example/\u{7}",
        ] {
            assert!(check_link(refused).is_err(), "{refused:?} was accepted");
        }
        let long = format!("https://example.com/{}", "a".repeat(ATTACHMENT_URL_MAX));
        assert!(check_link(&long).is_err());
    }

    #[test]
    fn picked_files_are_checked_at_the_byte_bounds_before_upload() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.resize(ATTACHMENT_IMAGE_MAX as usize, 0);
        let at_cap = classify(PickKind::Image, "shot.png", "image/png", &png).unwrap();
        assert_eq!(at_cap.kind, AttachmentKind::Image);
        assert_eq!(at_cap.media_type, "image/png");
        png.push(0);
        assert!(classify(PickKind::Image, "shot.png", "image/png", &png).is_err());
        // An image picked as a file goes by the image rules.
        assert_eq!(
            classify(PickKind::File, "a.gif", "", b"GIF89a....")
                .unwrap()
                .kind,
            AttachmentKind::Image
        );
        let text = vec![b'a'; ATTACHMENT_TEXT_MAX as usize];
        assert_eq!(
            classify(PickKind::File, "log.txt", "", &text)
                .unwrap()
                .media_type,
            "text/plain"
        );
        let over = vec![b'a'; ATTACHMENT_TEXT_MAX as usize + 1];
        assert!(classify(PickKind::File, "log.txt", "", &over).is_err());
        assert!(classify(PickKind::File, "x.bin", "", b"a\0b").is_err());
        assert!(classify(PickKind::File, "x.pdf", "", &[0xff, 0xfe, 0x00]).is_err());
        assert!(classify(PickKind::Image, "notes.txt", "", b"hello").is_err());
        assert!(classify(PickKind::File, "empty.txt", "", b"").is_err());
    }

    #[test]
    fn names_are_bounded_on_a_character_boundary() {
        let name = "é".repeat(ATTACHMENT_NAME_MAX);
        let bounded = bounded_name(&name);
        assert!(bounded.len() <= ATTACHMENT_NAME_MAX);
        assert!(bounded.chars().all(|c| c == 'é'));
        assert_eq!(bounded_name("\u{1b}[2Jshot.png"), "[2Jshot.png");
    }
}

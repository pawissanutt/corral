//! Attaching to a session's next message from the app root. Desktop and web open the system
//! picker in the press (a browser opens its file input only then) and await it under the root
//! owner; a terminal has no picker, so File opens a field for a path on this device, which the
//! client reads off the UI thread. Links are typed, checked and sent inline, never fetched. An
//! image or file pasted into a session's composer is checked and uploaded as a picked one is;
//! a file copied in a desktop file manager arrives as its path and is read as a typed one.
//! Only bytes, a base name and a sniffed type reach the client; a path never leaves it.
//! Each session keeps its own staged items, so what is staged on one session page never goes
//! with another session's message.

use corral_protocol::attachment::{ATTACHMENT_IMAGE_MAX, AttachmentKind};
use nsh_core::file_dialog::FileFilter;
use nsh_core::image::ImageData;
use nsh_core::picker::{FilePickRequest, PickRequest, PickedStatus};
use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::assistant::{self, AttachField, PickKind, Staged};
use crate::model::daemon::Link;
use crate::model::{Form, Id};

fn is_image(item: &Staged) -> bool {
    matches!(
        item,
        Staged::Upload {
            kind: AttachmentKind::Image,
            ..
        }
    )
}

/// Saved images are scaled to fit this edge and kept in a format every browser and server
/// takes (HEIC and TIFF become JPEG).
const IMAGE_EDGE: i32 = 4096;

/// What the file picker offers; anything else picked is still checked before upload.
const TEXT_EXTENSIONS: [&str; 20] = [
    "txt", "md", "log", "json", "jsonl", "csv", "tsv", "yaml", "yml", "toml", "xml", "html", "css",
    "rs", "ts", "js", "py", "go", "sh", "diff",
];

/// The images File also offers, and by which a path is read as an image.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

const NOT_RUNNING: &str =
    "Not attached: the app's connection to corrald is not running. Nothing was sent.";

impl AppState {
    /// What is staged for `session`'s next message, tracked.
    pub fn staged(&self, session: &Id) -> Vec<Staged> {
        self.ask_staged
            .with(|staged| staged.get(session).cloned().unwrap_or_default())
    }

    /// Forgets what is staged for `session` once its message carries it.
    pub fn clear_staged(&self, session: &Id) {
        self.ask_staged.update(|staged| {
            staged.remove(session);
        });
    }

    fn stage(&self, session: &Id, item: Staged) {
        self.ask_staged
            .update(|staged| staged.entry(session.clone()).or_default().push(item));
    }

    /// File (an image or a text file) for `session`: the system picker in this press on desktop
    /// and web, the path field in a terminal. Refused visibly when corrald cannot take an upload
    /// now.
    pub fn attach(&self, session: &Id, pick: PickKind) {
        if let Some(refusal) = self.upload_refusal() {
            self.toast(refusal);
            return;
        }
        if self.form == Form::Terminal {
            // NSH's terminal host answers no picker; the owner names a file on this device.
            self.open_attach_field(session, AttachField::Path(pick));
            return;
        }
        self.close_attach_field();
        let answer = match pick {
            PickKind::Image => {
                let mut request = PickRequest::new(false, 1);
                request.max_dimension = Some(DevicePixels(IMAGE_EDGE));
                pick_images(request)
            }
            PickKind::File => pick_files(
                FilePickRequest::new(true, 0)
                    .max_size(ATTACHMENT_IMAGE_MAX)
                    .filter(FileFilter::new(
                        "Images and text files",
                        IMAGE_EXTENSIONS.into_iter().chain(TEXT_EXTENSIONS),
                    )),
            ),
        };
        let app = *self;
        let session = session.clone();
        let source_revision = self.source_revision.get_untracked();
        self.root.run(|| {
            // Owned by the root: closing Ask while the picker is up keeps what is picked.
            let _ = spawn(async move {
                let files = answer.await;
                if app.source_revision.get_untracked() != source_revision {
                    app.toast("Not attached: the data source changed while the picker was open.");
                    return;
                }
                let mut too_large = Vec::new();
                let files = files.into_iter().filter_map(|file| {
                    if file.status == PickedStatus::TooLarge {
                        too_large.push(format!(
                            "{} not attached: too large ({} max)",
                            assistant::bounded_name(&file.name),
                            assistant::byte_size(ATTACHMENT_IMAGE_MAX)
                        ));
                        return None;
                    }
                    Some((file.name, file.mime, file.data))
                });
                let mut refusals = app.stage_files(&session, pick, files);
                refusals.extend(too_large);
                app.toast_refusals(refusals);
            });
        });
    }

    /// What the owner pasted into `session`'s composer as attachments: each image (a
    /// screenshot, an image copied from a page or an app) is checked and uploaded as a picked one
    /// is, any other pasted file as a text file, and each file copied in a file manager is read
    /// from its path on this device as a typed one is. Refused visibly when corrald cannot take an
    /// upload now.
    pub fn paste(&self, session: &Id, paste: &PasteEvent) -> Vec<String> {
        let before = self.staged_images(session);
        if let Some(refusal) = self.upload_refusal() {
            self.toast(refusal);
            return Vec::new();
        }
        let (images, others): (Vec<_>, Vec<_>) = paste
            .files
            .iter()
            .map(|file| {
                (
                    file.name.clone(),
                    file.mime.clone(),
                    ImageData::from(file.data.clone()),
                )
            })
            .partition(|(_, mime, data)| {
                mime.trim().starts_with("image/") || assistant::sniff_image(data.bytes()).is_some()
            });
        let mut refusals = self.stage_files(session, PickKind::Image, images);
        let mut refuse = |why: String| {
            if !refusals.contains(&why) {
                refusals.push(why);
            }
        };
        for why in self.stage_files(session, PickKind::File, others) {
            refuse(why);
        }
        for uri in &paste.uris {
            let outcome = match local_path(uri) {
                Some(path) => self.attach_path(session, PickKind::File, &path),
                None => Err("only a file on this device attaches".into()),
            };
            if let Err(why) = outcome {
                let name = uri.rsplit('/').next().unwrap_or(uri);
                refuse(format!(
                    "{} not attached: {why}",
                    assistant::bounded_name(name)
                ));
            }
        }
        self.toast_refusals(refusals);
        (before + 1..=self.staged_images(session))
            .map(assistant::image_marker)
            .collect()
    }

    /// How many images are staged for session `session`'s next message.
    pub(crate) fn staged_images(&self, session: &Id) -> usize {
        self.ask_staged.with_untracked(|staged| {
            staged
                .get(session)
                .into_iter()
                .flatten()
                .filter(|item| is_image(item))
                .count()
        })
    }

    pub fn fold_paste(&self, session: &Id, text: &str) -> String {
        if !assistant::is_long_paste(text) {
            return text.to_owned();
        }
        let mut k = 0;
        self.ask_pastes.update(|pastes| {
            let pastes = pastes.entry(session.clone()).or_default();
            pastes.push(text.to_owned());
            k = pastes.len();
        });
        assistant::paste_marker(k, text)
    }

    pub fn expand_pastes(&self, session: &Id, text: &str) -> String {
        self.ask_pastes.with_untracked(|pastes| {
            pastes.get(session).map_or_else(
                || text.to_owned(),
                |pastes| assistant::expand_pastes(text, pastes),
            )
        })
    }

    pub fn clear_pastes(&self, session: &Id) {
        self.ask_pastes.update(|pastes| {
            pastes.remove(session);
        });
    }

    fn toast_refusals(&self, refusals: Vec<String>) {
        if !refusals.is_empty() {
            self.toast(refusals.join(" · "));
        }
    }

    /// Opens the inline field in `session`'s composer, or closes it when that field is open.
    pub fn open_attach_field(&self, session: &Id, field: AttachField) {
        let open = self
            .ask_field
            .with_untracked(|open| open.as_ref() == Some(&(session.clone(), field)));
        if open {
            self.close_attach_field();
            return;
        }
        self.ask_field_text.set(String::new());
        self.ask_field_error.set(None);
        self.ask_field.set(Some((session.clone(), field)));
    }

    /// Closes the inline field and forgets what was typed in it.
    pub fn close_attach_field(&self) {
        self.ask_field.set(None);
        self.ask_field_text.set(String::new());
        self.ask_field_error.set(None);
    }

    /// Attaches what the open field holds: a link inline, or a path read by the client. On a
    /// refusal the field stays open with the reason under it.
    pub fn submit_attach_field(&self) {
        let Some((session, field)) = self.ask_field.get_untracked() else {
            return;
        };
        let text = self.ask_field_text.get_untracked();
        let outcome = match field {
            AttachField::Link => self.stage_link(&session, &text),
            AttachField::Path(pick) => self.attach_path(&session, pick, text.trim()),
        };
        match outcome {
            Ok(()) => self.close_attach_field(),
            Err(why) => self.ask_field_error.set(Some(why)),
        }
    }

    /// Removes a staged item from `session`'s next message, or from every session's when none
    /// is named; an upload's bytes and its unattached server copy are discarded.
    pub(crate) fn unattach(&self, session: Option<&Id>, key: &Id) {
        let mut removed = Vec::new();
        let mut images = Vec::new();
        self.ask_staged.update(|staged| {
            for (owner, items) in staged.iter_mut() {
                if session.is_some_and(|session| session != owner) {
                    continue;
                }
                if let Some(at) = items.iter().position(|item| item.key() == key.as_str()) {
                    if is_image(&items[at]) {
                        images.push((
                            owner.clone(),
                            1 + items[..at].iter().filter(|item| is_image(item)).count(),
                        ));
                    }
                    removed.push(items.remove(at));
                }
            }
            staged.retain(|_, items| !items.is_empty());
        });
        for (owner, k) in images {
            let draft = self.draft_of(&owner);
            let next = assistant::drop_image_marker(&draft, k);
            if next != draft {
                self.set_draft(owner, next);
            }
        }
        if self.fixture() {
            return;
        }
        for item in removed {
            if let Staged::Upload { upload, .. } = item {
                requests::attach_discard(upload.to_string());
            }
        }
    }

    /// Why an upload cannot start now, if it cannot.
    fn upload_refusal(&self) -> Option<String> {
        if self.fixture() {
            return Some(
                "The fixture preview attaches nothing; connect to corrald to attach.".into(),
            );
        }
        self.feed.with_untracked(|feed| {
            if !feed.capable(corral_protocol::attachment::ASSISTANT_ATTACH_V1) {
                Some(
                    "Not attached: this corrald does not accept attachments; upgrade corral."
                        .into(),
                )
            } else if feed.link != Link::Up || feed.session.is_none() {
                Some(
                    "Not attached: images and files upload only while corrald is reachable.".into(),
                )
            } else {
                None
            }
        })
    }

    /// Checks each picked or pasted file (its name, type and bytes) before anything is uploaded
    /// and stages the accepted ones for `session`, answering why each other one is not. An
    /// empty answer is a cancelled picker (or a portal that failed and logged it): nothing.
    fn stage_files(
        &self,
        session: &Id,
        pick: PickKind,
        files: impl IntoIterator<Item = (String, String, ImageData)>,
    ) -> Vec<String> {
        let mut refusals = Vec::new();
        for (name, mime, data) in files {
            let classified = match assistant::classify(pick, &name, &mime, data.bytes()) {
                Ok(classified) => classified,
                Err(why) => {
                    refusals.push(why);
                    continue;
                }
            };
            // A system picker can return after the user selected the isolated preview.
            if let Some(why) = self.upload_refusal() {
                refusals.push(why);
                break;
            }
            let upload = requests::new_id();
            let handed = requests::attach(requests::AttachRequest {
                upload: upload.clone(),
                kind: classified.kind.clone(),
                name: classified.name.clone(),
                media_type: classified.media_type,
                bytes: data,
            });
            if !handed {
                refusals.push(NOT_RUNNING.into());
                break;
            }
            self.stage(
                session,
                Staged::Upload {
                    upload: Id::from(upload),
                    kind: classified.kind,
                    name: classified.name,
                },
            );
        }
        refusals
    }

    fn stage_link(&self, session: &Id, text: &str) -> Result<(), String> {
        let mut url =
            assistant::check_link(text).map_err(|why| format!("not a web link: {why}"))?;
        // Only the scheme is case-insensitive; preserve the opaque URL remainder.
        if let Some(colon) = url.find(':') {
            url.replace_range(..colon, &url[..colon].to_ascii_lowercase());
        }
        self.ask_staged.update(|staged| {
            let staged = staged.entry(session.clone()).or_default();
            if !staged.iter().any(|item| item.key() == url) {
                staged.push(Staged::Link { url });
            }
        });
        Ok(())
    }

    /// Hands a path typed on this device to the client, which opens it once off the UI thread,
    /// checks the open handle (a regular file within the kind's bound) and uploads only its
    /// bytes, base name and sniffed type. The chip names the base name alone.
    #[cfg(not(target_arch = "wasm32"))]
    fn attach_path(&self, session: &Id, pick: PickKind, path: &str) -> Result<(), String> {
        if path.is_empty() {
            return Err("type the path of a file on this device".into());
        }
        let base = path.rsplit(['/', '\\']).next().unwrap_or_default();
        if base.is_empty() || base == "." || base == ".." {
            return Err("name a file, not a folder".into());
        }
        if let Some(refusal) = self.upload_refusal() {
            return Err(refusal);
        }
        // The client checks a path against its kind's bound before it reads it, so File names
        // an image by its extension.
        let image = base
            .rsplit_once('.')
            .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
        let pick = if image { PickKind::Image } else { pick };
        let upload = requests::new_id();
        let handed = requests::attach_path(requests::AttachPath {
            upload: upload.clone(),
            kind: pick.kind(),
            path: path.to_owned(),
        });
        if !handed {
            return Err(NOT_RUNNING.into());
        }
        let name = assistant::bounded_name(base);
        self.stage(
            session,
            Staged::Upload {
                upload: Id::from(upload),
                kind: pick.kind(),
                name,
            },
        );
        Ok(())
    }

    /// A browser reads no paths; its File uses the system picker instead.
    #[cfg(target_arch = "wasm32")]
    fn attach_path(&self, _session: &Id, _pick: PickKind, _text: &str) -> Result<(), String> {
        Err("a browser reads no paths; use File to pick one".into())
    }
}

/// The path a `file:` URL of a file on this device names, its escapes decoded; none for another
/// scheme, another host or escapes that are not UTF-8.
fn local_path(uri: &str) -> Option<String> {
    let scheme = uri.get(..7)?;
    if !scheme.eq_ignore_ascii_case("file://") {
        return None;
    }
    let rest = &uri[7..];
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let mut raw = rest.bytes();
    while let Some(byte) = raw.next() {
        if byte == b'%' {
            let hex = [raw.next()?, raw.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // Windows writes a drive after the root: `file:///C:/notes.txt`.
    let drive = path.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(2) == Some(&b':');
    Some(if cfg!(windows) && drive {
        path[1..].to_owned()
    } else {
        path
    })
}

#[cfg(test)]
mod tests {
    use super::local_path;

    /// A file copied in a file manager arrives as an escaped `file:` URL; read as written, a
    /// name with a space named no file.
    #[test]
    fn a_copied_file_s_url_names_its_path_on_this_device() {
        assert_eq!(
            local_path("file:///home/me/My%20shot.png").as_deref(),
            Some("/home/me/My shot.png")
        );
        assert_eq!(
            local_path("FILE://localhost/tmp/a.txt").as_deref(),
            Some("/tmp/a.txt")
        );
        assert_eq!(local_path("file://server/share/a.txt"), None);
        assert_eq!(local_path("https://example.com/a.png"), None);
        assert_eq!(local_path("file:///bad%zz"), None);
    }
}

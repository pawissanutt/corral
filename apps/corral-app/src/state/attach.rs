//! Attaching to a session's next message from the app root. Desktop and web open the system
//! picker in the press (a browser opens its file input only then) and await it under the root
//! owner; a terminal has no picker, so Screenshot and File open a field for a path on this device,
//! which the client reads off the UI thread. Links are typed, checked and sent inline, never
//! fetched. An image pasted into a session's composer is checked and uploaded as a picked one is.
//! Only bytes, a base name and a sniffed type reach the client; a typed path never leaves it.
//! Each session keeps its own staged items, so what is staged on one session page never goes
//! with another session's message.

use corral_protocol::attachment::ATTACHMENTS_PER_MESSAGE;
use nsh_core::file_dialog::FileFilter;
use nsh_core::image::ImageData;
use nsh_core::picker::{FilePickRequest, PickRequest, PickedFile};
use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::assistant::{self, AttachField, PickKind, Staged};
use crate::model::daemon::Link;
use crate::model::{Form, Id};

/// Saved images are scaled to fit this edge and kept in a format every browser and server
/// takes (HEIC and TIFF become JPEG).
const IMAGE_EDGE: i32 = 4096;

/// What the file picker offers; anything else picked is still checked before upload.
const TEXT_EXTENSIONS: [&str; 20] = [
    "txt", "md", "log", "json", "jsonl", "csv", "tsv", "yaml", "yml", "toml", "xml", "html", "css",
    "rs", "ts", "js", "py", "go", "sh", "diff",
];

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

    fn staged_count(&self, session: &Id) -> usize {
        self.ask_staged
            .with_untracked(|staged| staged.get(session).map_or(0, Vec::len))
    }

    fn stage(&self, session: &Id, item: Staged) {
        self.ask_staged
            .update(|staged| staged.entry(session.clone()).or_default().push(item));
    }

    /// Screenshot or File for `session`: the system picker in this press on desktop and web,
    /// the path field in a terminal. Refused visibly when corrald cannot take an upload now.
    pub fn attach(&self, session: &Id, pick: PickKind) {
        if let Some(refusal) = self.upload_refusal(session) {
            self.toast(refusal);
            return;
        }
        let room = ATTACHMENTS_PER_MESSAGE.saturating_sub(self.staged_count(session));
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
                FilePickRequest::new(room > 1, u32::try_from(room).unwrap_or(1))
                    .filter(FileFilter::new("Text files", TEXT_EXTENSIONS)),
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
                let files = files
                    .into_iter()
                    .map(|file: PickedFile| (file.name, file.mime, file.data));
                let refusals = app.stage_files(&session, pick, files);
                app.toast_refusals(refusals);
            });
        });
    }

    /// What the owner pasted into `session`'s composer: each image (a screenshot, an image
    /// copied from a page or an app) is checked and uploaded as a picked one is, any other
    /// pasted file as a text file. The pasted text stays the field's; a paste without files
    /// stages nothing. Refused visibly when corrald cannot take an upload now, and for files
    /// copied in a file manager, which arrive as paths alone.
    pub fn paste(&self, session: &Id, paste: &PasteEvent) {
        if paste.files.is_empty() {
            if !paste.uris.is_empty() {
                self.toast(
                    "Not attached: a copied file arrives as its path alone; attach it with \
                     Screenshot or File, or paste the image itself.",
                );
            }
            return;
        }
        if let Some(refusal) = self.upload_refusal(session) {
            self.toast(refusal);
            return;
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
        refusals.extend(self.stage_files(session, PickKind::File, others));
        self.toast_refusals(refusals);
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
            AttachField::Path(pick) => self.attach_path(&session, pick, &text),
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
        self.ask_staged.update(|staged| {
            for (owner, items) in staged.iter_mut() {
                if session.is_some_and(|session| session != owner) {
                    continue;
                }
                if let Some(at) = items.iter().position(|item| item.key() == key.as_str()) {
                    removed.push(items.remove(at));
                }
            }
            staged.retain(|_, items| !items.is_empty());
        });
        if self.fixture() {
            return;
        }
        for item in removed {
            if let Staged::Upload { upload, .. } = item {
                requests::attach_discard(upload.to_string());
            }
        }
    }

    /// Why an upload for `session` cannot start now, if it cannot.
    fn upload_refusal(&self, session: &Id) -> Option<String> {
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
                    "Not attached: screenshots and files upload only while corrald is reachable."
                        .into(),
                )
            } else if self.staged_count(session) >= ATTACHMENTS_PER_MESSAGE {
                Some(format!(
                    "Not attached: {ATTACHMENTS_PER_MESSAGE} attachments per message."
                ))
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
            if self.staged_count(session) >= ATTACHMENTS_PER_MESSAGE {
                refusals.push(format!(
                    "{name} not attached: {ATTACHMENTS_PER_MESSAGE} attachments per message"
                ));
                continue;
            }
            let classified = match assistant::classify(pick, &name, &mime, data.bytes()) {
                Ok(classified) => classified,
                Err(why) => {
                    refusals.push(why);
                    continue;
                }
            };
            // A system picker can return after the user selected the isolated preview.
            if let Some(why) = self.upload_refusal(session) {
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
        let mut full = false;
        self.ask_staged.update(|staged| {
            let staged = staged.entry(session.clone()).or_default();
            if staged.iter().any(|item| item.key() == url) {
                return;
            }
            if staged.len() >= ATTACHMENTS_PER_MESSAGE {
                full = true;
                return;
            }
            staged.push(Staged::Link { url: url.clone() });
        });
        if full {
            return Err(format!("{ATTACHMENTS_PER_MESSAGE} attachments per message"));
        }
        Ok(())
    }

    /// Hands a path typed on this device to the client, which opens it once off the UI thread,
    /// checks the open handle (a regular file within the kind's bound) and uploads only its
    /// bytes, base name and sniffed type. The chip names the base name alone.
    #[cfg(not(target_arch = "wasm32"))]
    fn attach_path(&self, session: &Id, pick: PickKind, text: &str) -> Result<(), String> {
        let path = text.trim();
        if path.is_empty() {
            return Err("type the path of a file on this device".into());
        }
        let base = path.rsplit(['/', '\\']).next().unwrap_or_default();
        if base.is_empty() || base == "." || base == ".." {
            return Err("name a file, not a folder".into());
        }
        if let Some(refusal) = self.upload_refusal(session) {
            return Err(refusal);
        }
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

    /// A browser reads no paths; its File and Screenshot use the system picker instead.
    #[cfg(target_arch = "wasm32")]
    fn attach_path(&self, _session: &Id, _pick: PickKind, _text: &str) -> Result<(), String> {
        Err("a browser reads no paths; use Screenshot or File to pick one".into())
    }
}

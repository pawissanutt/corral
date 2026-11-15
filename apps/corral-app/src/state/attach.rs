//! Attaching to the next message from the app root. Desktop and web open the system picker in
//! the press (a browser opens its file input only then) and await it under the root owner; a
//! terminal has no picker, so Screenshot and File open a field for a path on this device, which
//! the client reads off the UI thread. Links are typed, checked and sent inline, never fetched.
//! Only bytes, a base name and a sniffed type reach the client; a typed path never leaves it.

use corral_protocol::attachment::ATTACHMENTS_PER_MESSAGE;
use nsh_core::file_dialog::FileFilter;
use nsh_core::picker::{FilePickRequest, PickRequest, PickedFile};
use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::assistant::{self, AttachField, PickKind, Staged};
use crate::model::daemon::Link;
use crate::model::{Authority, Form, Id};

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
    /// Screenshot or File: the system picker in this press on desktop and web, the path field
    /// in a terminal. Refused visibly when corrald cannot take an upload now.
    pub fn attach(&self, pick: PickKind) {
        if let Some(refusal) = self.upload_refusal() {
            self.toast(refusal);
            return;
        }
        let room = ATTACHMENTS_PER_MESSAGE.saturating_sub(self.ask_staged.with_untracked(Vec::len));
        if self.form == Form::Terminal {
            // NSH's terminal host answers no picker; the owner names a file on this device.
            self.open_attach_field(AttachField::Path(pick));
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
        self.root.run(|| {
            // Owned by the root: closing Ask while the picker is up keeps what is picked.
            let _ = spawn(async move {
                let files = answer.await;
                app.stage_picked(pick, files);
            });
        });
    }

    /// Opens the inline field, or closes it when that field is already open.
    pub fn open_attach_field(&self, field: AttachField) {
        if self.ask_field.get_untracked() == Some(field) {
            self.close_attach_field();
            return;
        }
        self.ask_field_text.set(String::new());
        self.ask_field_error.set(None);
        self.ask_field.set(Some(field));
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
        let Some(field) = self.ask_field.get_untracked() else {
            return;
        };
        let text = self.ask_field_text.get_untracked();
        let outcome = match field {
            AttachField::Link => self.stage_link(&text),
            AttachField::Path(pick) => self.attach_path(pick, &text),
        };
        match outcome {
            Ok(()) => self.close_attach_field(),
            Err(why) => self.ask_field_error.set(Some(why)),
        }
    }

    /// Removes a staged item; an upload's bytes and its unattached server copy are discarded.
    pub(super) fn unattach(&self, key: &Id) {
        let mut removed = None;
        self.ask_staged.update(|staged| {
            if let Some(at) = staged.iter().position(|item| item.key() == key.as_str()) {
                removed = Some(staged.remove(at));
            }
        });
        if let Some(Staged::Upload { upload, .. }) = removed {
            requests::attach_discard(upload.to_string());
        }
    }

    /// Persisted acceptance transfers these staged keys to their message.
    pub(crate) fn sent_attachments(&self, carried: &[String]) {
        if carried.is_empty() {
            return;
        }
        self.ask_staged
            .update(|staged| staged.retain(|item| !carried.iter().any(|key| key == item.key())));
    }

    /// Why an upload cannot start now, if it cannot.
    fn upload_refusal(&self) -> Option<String> {
        if !untracked(|| self.daemon()) {
            return Some(
                "The fixture preview attaches nothing; connect to corrald to attach.".into(),
            );
        }
        self.feed.with_untracked(|feed| {
            let granted = feed
                .session
                .as_ref()
                .is_some_and(|session| session.authority.assistant);
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
            } else if !granted {
                Some(Authority::Assistant.refusal().into())
            } else if self.ask_staged.with_untracked(Vec::len) >= ATTACHMENTS_PER_MESSAGE {
                Some(format!(
                    "Not attached: {ATTACHMENTS_PER_MESSAGE} attachments per message."
                ))
            } else {
                None
            }
        })
    }

    /// Checks each picked file before anything is uploaded and stages the accepted ones. An
    /// empty answer is a cancelled picker (or a portal that failed and logged it): nothing.
    fn stage_picked(&self, pick: PickKind, files: Vec<PickedFile>) {
        let mut refusals = Vec::new();
        for file in files {
            if self.ask_staged.with_untracked(Vec::len) >= ATTACHMENTS_PER_MESSAGE {
                refusals.push(format!(
                    "{} not attached: {ATTACHMENTS_PER_MESSAGE} attachments per message",
                    file.name
                ));
                continue;
            }
            let checked = assistant::classify(pick, &file.name, &file.mime, file.data.bytes());
            let classified = match checked {
                Ok(classified) => classified,
                Err(why) => {
                    refusals.push(why);
                    continue;
                }
            };
            let upload = requests::new_id();
            let handed = requests::attach(requests::AttachRequest {
                upload: upload.clone(),
                kind: classified.kind.clone(),
                name: classified.name.clone(),
                media_type: classified.media_type,
                bytes: file.data,
            });
            if !handed {
                refusals.push(NOT_RUNNING.into());
                break;
            }
            self.ask_staged.update(|staged| {
                staged.push(Staged::Upload {
                    upload: Id::from(upload),
                    kind: classified.kind,
                    name: classified.name,
                })
            });
        }
        if !refusals.is_empty() {
            self.toast(refusals.join(" · "));
        }
    }

    fn stage_link(&self, text: &str) -> Result<(), String> {
        let mut url =
            assistant::check_link(text).map_err(|why| format!("not a web link: {why}"))?;
        // Only the scheme is case-insensitive; preserve the opaque URL remainder.
        if let Some(colon) = url.find(':') {
            url.replace_range(..colon, &url[..colon].to_ascii_lowercase());
        }
        let mut full = false;
        self.ask_staged.update(|staged| {
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
    fn attach_path(&self, pick: PickKind, text: &str) -> Result<(), String> {
        let path = text.trim();
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
        self.ask_staged.update(|staged| {
            staged.push(Staged::Upload {
                upload: Id::from(upload),
                kind: pick.kind(),
                name,
            })
        });
        Ok(())
    }

    /// A browser reads no paths; its File and Screenshot use the system picker instead.
    #[cfg(target_arch = "wasm32")]
    fn attach_path(&self, _pick: PickKind, _text: &str) -> Result<(), String> {
        Err("a browser reads no paths; use Screenshot or File to pick one".into())
    }
}

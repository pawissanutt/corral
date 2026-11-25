//! Download: the whole artifact's original bytes saved through the platform's own way to save a
//! file (the save dialog on a desktop, a browser download on the web, the Downloads folder where
//! no dialog exists, as in a terminal). The client reads the bytes from corrald page by page
//! into the sink, which keeps them in a private temporary file or Blob parts until the read is
//! complete and verified, so a failed or cancelled download leaves no file behind.
//!
//! test ids: "btn-download-artifact", "btn-cancel-download", "artifact-download".

use std::{cell::RefCell, rc::Rc};

use nsh_core::prelude::*;

use crate::client::download::{SaveArtifact, SaveOutput};
use crate::client::requests::{self, ClientRequest};
use crate::model::Form;
use crate::model::artifacts::Save;
use crate::model::artifacts::download::{CANCEL_DOWNLOAD, DOWNLOAD, Saving};
use crate::model::daemon::Feed;
use crate::model::health::DataState;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::text::dim;

#[cfg(not(target_arch = "wasm32"))]
#[path = "save_native.rs"]
mod native;

/// The finder's one download: which artifact it is for and where it stands. It lives as long
/// as the finder, so moving between the results and full size keeps it; leaving the page drops
/// the status, and with it the transfer.
#[derive(Clone, Copy)]
pub(super) struct Downloads {
    reference: Signal<Option<String>>,
    status: Signal<Saving>,
    // The cleanup callback also holds this slot: signals may already be gone at disposal.
    reading: Signal<Rc<RefCell<Option<String>>>>,
}

pub(super) fn downloads() -> Downloads {
    let reading = Rc::new(RefCell::new(None));
    let held = reading.clone();
    // The client notices a gone status only when the next page arrives; leaving the finder
    // tells it now, so the transfer and its temporary file end at once.
    cleanup(move || {
        if let Some(id) = held.take() {
            _ = requests::send(ClientRequest::CancelSave(id));
        }
    });
    Downloads {
        reference: signal(None),
        status: signal(Saving::Idle),
        reading: signal(reading),
    }
}

impl Downloads {
    /// Where `reference`'s download stands; another artifact's never shows under it.
    fn of(self, reference: &str) -> Saving {
        if self
            .reference
            .with(|shown| shown.as_deref() == Some(reference))
        {
            self.status.get()
        } else {
            Saving::Idle
        }
    }
}

/// Download, or Cancel while this artifact's bytes are read.
pub(super) fn control(app: AppState, downloads: Downloads, save: &Save) -> AnyElement {
    let reference = save.reference.clone();
    let reading = computed(move || match downloads.of(&reference) {
        Saving::Reading { id, .. } => Some(id),
        _ => None,
    });
    let save = save.clone();
    dynamic(move || match reading.get() {
        Some(id) => button(
            app,
            CANCEL_DOWNLOAD,
            ButtonKind::Ghost,
            "btn-cancel-download",
            move || cancel(downloads, &id),
        ),
        None => {
            let save = save.clone();
            button(
                app,
                DOWNLOAD,
                ButtonKind::Ghost,
                "btn-download-artifact",
                move || start(app, downloads, save.clone()),
            )
        }
    })
    .into_element()
}

/// Where this artifact's download stands, under the actions; nothing while none is under way.
pub(super) fn note(app: AppState, downloads: Downloads, reference: &str) -> AnyElement {
    let reference = reference.to_owned();
    let words = computed(move || downloads.of(&reference).note());
    dynamic(move || match words.get() {
        Some(words) => dim(app, words).test_id("artifact-download").into_element(),
        None => view().into_element(),
    })
    .into_element()
}

fn cancel(downloads: Downloads, id: &str) {
    _ = requests::send(ClientRequest::CancelSave(id.to_owned()));
    downloads.status.set(Saving::Idle);
}

fn start(app: AppState, downloads: Downloads, save: Save) {
    // One download at a time: another artifact's read gives way; a dialog or a file being put
    // in place finishes first.
    match downloads.status.get_untracked() {
        Saving::Reading { id, .. } => cancel(downloads, &id),
        Saving::Choosing | Saving::Finishing => return,
        Saving::Idle | Saving::Saved(_) | Saving::Failed(_) => {}
    }
    downloads.reference.set(Some(save.reference.clone()));
    let status = downloads.status;
    let connected =
        app.data.get_untracked() == DataState::Daemon && app.feed.with_untracked(Feed::ready);
    if !connected {
        status.set(Saving::Failed(
            "Downloading needs a connection to corrald.".into(),
        ));
        return;
    }
    status.set(Saving::Choosing);
    let form = app.form;
    // Not the button's: a redraw must not drop a download under way. Leaving the finder drops
    // the status, which ends it.
    app.root.run(|| {
        spawn(async move {
            let output = match sink(&save.file_name, form).await {
                Ok(Some(output)) => output,
                Ok(None) => {
                    status.try_set(Saving::Idle);
                    return;
                }
                Err(problem) => {
                    status.try_set(Saving::Failed(problem));
                    return;
                }
            };
            if status.try_get() != Some(Saving::Choosing) {
                return;
            }
            let id = requests::new_id();
            if let Some(reading) = downloads.reading.try_get() {
                reading.replace(Some(id.clone()));
            }
            status.set(Saving::Reading {
                id: id.clone(),
                percent: 0,
            });
            let sent = requests::send(ClientRequest::SaveArtifact(SaveArtifact {
                id,
                reference: save.reference,
                bytes: save.bytes,
                sha256: save.sha256,
                status,
                output,
            }));
            if !sent {
                status.try_set(Saving::Failed(
                    "The connection to corrald is unavailable.".into(),
                ));
            }
        });
    });
}

/// Where the bytes go: none when the owner cancels the save dialog.
#[cfg(not(target_arch = "wasm32"))]
async fn sink(name: &str, form: Form) -> Result<Option<SaveOutput>, String> {
    native::sink(name, form).await
}

#[cfg(target_arch = "wasm32")]
async fn sink(name: &str, _: Form) -> Result<Option<SaveOutput>, String> {
    web::WebSave::new(name).map(|save| Some(Box::new(save) as SaveOutput))
}

#[cfg(target_arch = "wasm32")]
mod web {
    use js_sys::{Array, Function, Uint8Array};
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
    use web_sys::{Blob, BlobPropertyBag, HtmlElement, Url};

    use crate::client::download::{SaveFinish, SaveSink};

    /// The bytes as Blob parts the browser holds, outside the app's memory, until the read is
    /// complete; then one browser download under the suggested name.
    pub(super) struct WebSave {
        name: String,
        parts: Array,
    }

    impl WebSave {
        pub(super) fn new(name: &str) -> Result<Self, String> {
            Ok(Self {
                name: name.to_owned(),
                parts: Array::new(),
            })
        }

        fn save(self) -> Result<String, String> {
            let window = web_sys::window().ok_or("No browser window is available.")?;
            let document = window
                .document()
                .ok_or("No browser document is available.")?;
            let props = BlobPropertyBag::new();
            props.set_type("application/octet-stream");
            let blob = Blob::new_with_blob_sequence_and_options(&self.parts, &props)
                .map_err(|_| "Could not assemble the download.")?;
            let url = Url::create_object_url_with_blob(&blob)
                .map_err(|_| "Could not create a browser URL for the download.")?;
            let retire = url.clone();
            let revoke: Function = Closure::once_into_js(move || {
                let _ = Url::revoke_object_url(&retire);
            })
            .unchecked_into();
            let link = document
                .create_element("a")
                .map_err(|_| "Could not start the browser download.")?;
            let started = link
                .set_attribute("href", &url)
                .and_then(|()| link.set_attribute("download", &self.name))
                .ok()
                .and_then(|()| link.dyn_into::<HtmlElement>().ok())
                .map(|link| link.click());
            if started.is_none() {
                let _ = revoke.call0(&JsValue::UNDEFINED);
                return Err("Could not start the browser download.".into());
            }
            // The browser reads the URL after the click returns, so it is retired a minute
            // later; without a timer it stays until the page goes.
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&revoke, 60_000);
            // A page is not told when the browser's download completes.
            Ok(format!(
                "as {}: handed to the browser's downloads",
                self.name
            ))
        }
    }

    impl SaveSink for WebSave {
        fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
            let buffers = Array::new();
            buffers.push(&Uint8Array::from(bytes));
            let part = Blob::new_with_u8_array_sequence(&buffers)
                .map_err(|_| "Could not stage the download.")?;
            self.parts.push(&part);
            Ok(())
        }

        fn finish(self: Box<Self>) -> SaveFinish {
            Box::pin(async move { self.save() })
        }
    }
}

//! Download: the whole artifact's original bytes saved through the platform's own way to save a
//! file: the save dialog on a desktop, the Downloads folder where no dialog exists (a terminal),
//! the browser's save picker on the web, else a browser download of bounded size. The client
//! reads the bytes from corrald page by page, writing each to the sink before asking for the
//! next; the sink keeps them in a private temporary file, the picked file's uncommitted stream
//! or bounded Blob parts until the read is complete and verified, so a failed or cancelled
//! download leaves no file behind.
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
#[cfg(target_arch = "wasm32")]
#[path = "save_web.rs"]
mod web;

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
            let output = match sink(&save.file_name, save.bytes, form).await {
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
async fn sink(name: &str, _: u64, form: Form) -> Result<Option<SaveOutput>, String> {
    native::sink(name, form).await
}

#[cfg(target_arch = "wasm32")]
async fn sink(name: &str, bytes: u64, _: Form) -> Result<Option<SaveOutput>, String> {
    web::sink(name, bytes).await
}

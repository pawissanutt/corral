//! Open in browser for a standalone HTML artifact. The press reserves the platform's isolated
//! preview (a sandboxed copy in a private temporary file natively, a blank tab without an
//! opener on the web, reserved while the press still counts as the owner's), then reads the
//! whole saved artifact from corrald through the download's transfer, which hands the preview
//! nothing to launch unless every byte arrived and matched the catalog's size and SHA-256. The
//! preview never gets the app's origin, a worktree path or a partial head.
//!
//! test ids: "btn-open-html-artifact", "btn-cancel-html-artifact", "artifact-browser".

use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

use nsh_core::prelude::*;

use crate::client::download::SaveArtifact;
use crate::client::html::HtmlSave;
use crate::client::requests::{self, ClientRequest};
use crate::model::artifacts::Save;
use crate::model::artifacts::browser::{
    self, CANCEL_OPEN, OPEN_IN_BROWSER, SANDBOXED, UNSUPPORTED,
};
use crate::model::artifacts::download::Saving;
use crate::model::daemon::Feed;
use crate::model::health::DataState;
use crate::screens::agents::{HtmlStop, html_target};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::text::dim;

/// Whether this client can open a browser at all.
const SUPPORTED: bool = cfg!(any(
    target_arch = "wasm32",
    target_os = "linux",
    target_os = "macos"
));

/// The finder's one browser open, as long-lived as the finder: leaving it ends a read under
/// way and refuses a launch still waiting.
#[derive(Clone, Copy)]
pub(super) struct Opens {
    reference: Signal<Option<String>>,
    status: Signal<Saving>,
    // The cleanup callback also holds this slot: signals may already be gone at disposal.
    reading: Signal<Rc<Held>>,
}

/// The read under way and its preview's stop, for the cleanup when the finder goes.
#[derive(Default)]
struct Held(RefCell<Option<(String, HtmlStop)>>);

impl PartialEq for Held {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

pub(super) fn opens() -> Opens {
    let reading = Rc::new(Held::default());
    let held = reading.clone();
    cleanup(move || {
        if let Some((id, stop)) = held.0.take() {
            stop.store(true, Ordering::Relaxed);
            _ = requests::send(ClientRequest::CancelSave(id));
        }
    });
    Opens {
        reference: signal(None),
        status: signal(Saving::Idle),
        reading: signal(reading),
    }
}

impl Opens {
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

/// Open in browser, or Cancel while the page is read; on a client without a browser, nothing.
pub(super) fn control(app: AppState, opens: Opens, save: &Save) -> Option<AnyElement> {
    if !SUPPORTED {
        return None;
    }
    let reference = save.reference.clone();
    let reading = computed(move || match opens.of(&reference) {
        Saving::Reading { id, .. } => Some(id),
        _ => None,
    });
    let save = save.clone();
    Some(
        dynamic(move || match reading.get() {
            Some(id) => button(
                app,
                CANCEL_OPEN,
                ButtonKind::Ghost,
                "btn-cancel-html-artifact",
                move || cancel(opens, &id),
            ),
            None => {
                let save = save.clone();
                button(
                    app,
                    OPEN_IN_BROWSER,
                    ButtonKind::Ghost,
                    "btn-open-html-artifact",
                    move || start(app, opens, save.clone()),
                )
            }
        })
        .into_element(),
    )
}

/// What the open does, then where it stands; on a client without a browser, the fallback.
pub(super) fn note(app: AppState, opens: Opens, reference: &str) -> AnyElement {
    if !SUPPORTED {
        return dim(app, UNSUPPORTED)
            .test_id("artifact-browser")
            .into_element();
    }
    let reference = reference.to_owned();
    let words = computed(move || browser::note(&opens.of(&reference)));
    dynamic(move || {
        dim(app, words.get().unwrap_or_else(|| SANDBOXED.to_owned()))
            .test_id("artifact-browser")
            .into_element()
    })
    .into_element()
}

fn cancel(opens: Opens, id: &str) {
    _ = requests::send(ClientRequest::CancelSave(id.to_owned()));
    if let Some(reading) = opens.reading.try_get()
        && let Some((_, stop)) = reading.0.take()
    {
        stop.store(true, Ordering::Relaxed);
    }
    opens.status.set(Saving::Idle);
}

/// Reserves the preview within the press (a browser keeps a new tab only then), then asks for
/// the whole artifact.
fn start(app: AppState, opens: Opens, save: Save) {
    match opens.status.get_untracked() {
        Saving::Reading { id, .. } => cancel(opens, &id),
        Saving::Choosing | Saving::Finishing => return,
        Saving::Idle | Saving::Saved(_) | Saving::Failed(_) => {}
    }
    opens.reference.set(Some(save.reference.clone()));
    let status = opens.status;
    if let Some(why) = browser::refusal(&save) {
        status.set(Saving::Failed(why));
        return;
    }
    let connected =
        app.data.get_untracked() == DataState::Daemon && app.feed.with_untracked(Feed::ready);
    if !connected {
        status.set(Saving::Failed(
            "Opening a page needs a connection to corrald.".into(),
        ));
        return;
    }
    let stop = HtmlStop::default();
    let output = match html_target(stop.clone()) {
        Ok(output) => output,
        Err(problem) => {
            status.set(Saving::Failed(problem));
            return;
        }
    };
    let id = requests::new_id();
    if let Some(reading) = opens.reading.try_get() {
        reading.0.replace(Some((id.clone(), stop)));
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
        output: Box::new(HtmlSave(output)),
    }));
    if !sent {
        status.set(Saving::Failed(
            "The connection to corrald is unavailable.".into(),
        ));
    }
}

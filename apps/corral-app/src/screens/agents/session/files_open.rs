//! Open a complete HTML snapshot in a separate sandboxed browser document.
use std::{
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use nsh_core::prelude::*;

use crate::client::{
    html::{OpenHtml, Output},
    requests::{self, ClientRequest},
};
use crate::model::{Id, daemon::Feed, session::files_large::HtmlProgress};
use crate::state::AppState;
use crate::ui::{
    kit::button::{ButtonKind, button},
    tokens::{Token, color},
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "files_native.rs"]
mod native;

pub(super) fn progress() -> Signal<HtmlProgress> {
    signal(HtmlProgress::Idle)
}

pub(super) fn browser_button(
    app: AppState,
    progress: Signal<HtmlProgress>,
    session: &Id,
    path: &str,
    source: Option<&str>,
    can_fetch: bool,
) -> Option<AnyElement> {
    let available = can_fetch || source.is_some();
    if !cfg!(any(
        target_arch = "wasm32",
        target_os = "linux",
        target_os = "macos"
    )) {
        return available
            .then(|| note("Open this preview in a web session to launch it in a browser."));
    }
    let session = Rc::<str>::from(session.as_str());
    let path = Rc::<str>::from(path);
    let source = source.filter(|_| !can_fetch).map(Rc::<str>::from);
    // Only the label observes percentage changes, so the Cancel control keeps its focus.
    let controls = computed(move || match progress.get() {
        HtmlProgress::Reading { id, .. } => HtmlProgress::Reading { id, percent: 0 },
        state => state,
    });
    Some(dynamic(move || {
        match controls.get() {
        HtmlProgress::Reading { id, .. } => Some(view().flex_col()
            .child(text(move || match progress.get() {
                HtmlProgress::Reading { percent, .. } => format!("Preparing HTML snapshot… {percent}%"),
                _ => String::new(),
            }).text_color(color(Token::Ink2)))
            .child(button(app, "Cancel HTML open", ButtonKind::Ghost, "btn-cancel-html-browser", move || {
                if matches!(progress.get_untracked(), HtmlProgress::Reading { id: current, .. } if current == id) {
                    progress.set(HtmlProgress::Idle);
                    _ = requests::send(ClientRequest::CancelHtml(id.clone()));
                }
            })).into_element()),
        HtmlProgress::Launching => Some(note("Launching browser…")),
        state => {
            let mut controls = view().flex_col();
            if let HtmlProgress::Failed(error) = state {
                controls = controls.child(note(format!("Could not open HTML: {error}")));
            } else if !available {
                return None;
            }
            if available {
                let (session, path, source) = (session.clone(), path.clone(), source.clone());
                controls = controls
                    .child(note("Opens a sandboxed copy. Inline scripts can run and may navigate the frame to external sites. Open only files you trust."))
                    .child(button(app, "Open in browser", ButtonKind::Ghost, "btn-open-html-browser", move || {
                        start(app, progress, &session, &path, source.as_deref());
                    }));
            }
            Some(controls.into_element())
        }
    }
    }))
}

fn start(
    app: AppState,
    status: Signal<HtmlProgress>,
    session: &str,
    path: &str,
    source: Option<&str>,
) {
    if matches!(
        status.get_untracked(),
        HtmlProgress::Reading { .. } | HtmlProgress::Launching
    ) {
        return;
    }
    let result = (|| {
        if !app.feed.with_untracked(Feed::ready) {
            return Err("The daemon is offline.".into());
        }
        let mut output = target()?;
        if let Some(source) = source {
            output.append(source.as_bytes())?;
            status.set(HtmlProgress::Launching);
            // Even a small legacy preview must be cleaned up if its Files view goes away.
            app.root.run(|| {
                spawn(async move {
                    let result = output.finish().await;
                    status.try_set(match result {
                        Ok(()) => HtmlProgress::Idle,
                        Err(problem) => HtmlProgress::Failed(problem),
                    });
                })
            });
        } else {
            let id = requests::new_id();
            status.set(HtmlProgress::Reading {
                id: id.clone(),
                percent: 0,
            });
            if !requests::send(ClientRequest::OpenHtml(OpenHtml {
                id,
                session: session.to_owned(),
                path: path.to_owned(),
                status,
                output,
            })) {
                return Err("The daemon connection is unavailable.".into());
            }
        }
        Ok(())
    })();
    if let Err(problem) = result {
        status.set(HtmlProgress::Failed(problem));
    }
}

fn note(words: impl Into<String>) -> AnyElement {
    text(words.into())
        .text_color(color(Token::Ink2))
        .into_element()
}

const PREFIX: &[u8] = concat!(
    "<!doctype html><meta charset=utf-8><title>Worktree HTML preview</title>",
    "<style>body{margin:0}iframe{display:block;border:0;width:100vw;height:100vh}</style>",
    "<iframe title=\"Worktree HTML preview\" sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" srcdoc=\"",
    "&lt;!doctype html&gt;&lt;meta charset=utf-8&gt;",
    "&lt;meta http-equiv='Content-Security-Policy' content=&quot;default-src 'none'; ",
    "script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'&quot;&gt;",
).as_bytes();
const SUFFIX: &[u8] = b"\"></iframe>";

fn escape(bytes: &[u8]) -> Vec<u8> {
    // This escapes ONE attribute; the browser parses the decoded value in a sandboxed frame.
    let mut output = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'&' => output.extend_from_slice(b"&amp;"),
            b'"' => output.extend_from_slice(b"&#34;"),
            b'<' => output.extend_from_slice(b"&lt;"),
            b'>' => output.extend_from_slice(b"&gt;"),
            _ => output.push(byte),
        }
    }
    output
}

#[cfg(any(target_arch = "wasm32", target_os = "linux", target_os = "macos"))]
static OPEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(any(target_arch = "wasm32", target_os = "linux", target_os = "macos"))]
struct Slot;
#[cfg(any(target_arch = "wasm32", target_os = "linux", target_os = "macos"))]
impl Slot {
    fn reserve() -> Result<Self, String> {
        OPEN.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            (count < 4).then_some(count + 1)
        })
        .map_err(|_| "Four recent previews are still opening. Try again shortly.".to_owned())?;
        Ok(Self)
    }
}
#[cfg(any(target_arch = "wasm32", target_os = "linux", target_os = "macos"))]
impl Drop for Slot {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn target() -> Result<Output, String> {
    native::NativePreview::new().map(|preview| Box::new(preview) as Output)
}

#[cfg(not(any(target_arch = "wasm32", target_os = "linux", target_os = "macos")))]
fn target() -> Result<Output, String> {
    Err("This app cannot launch an HTML preview on this device.".into())
}

#[cfg(target_arch = "wasm32")]
fn target() -> Result<Output, String> {
    web::WebPreview::new().map(|preview| Box::new(preview) as Output)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use js_sys::{Array, Function, Uint8Array};
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
    use web_sys::{Blob, BlobPropertyBag, Url, Window};

    use crate::client::html::{HtmlFinish, HtmlSink};

    use super::{PREFIX, SUFFIX, Slot, escape};

    pub(super) struct WebPreview {
        opener: Window,
        popup: Window,
        parts: Array,
        slot: Option<Slot>,
        launched: bool,
    }

    impl WebPreview {
        pub(super) fn new() -> Result<Self, String> {
            let slot = Slot::reserve()?;
            let opener = web_sys::window().ok_or("No browser window is available.")?;
            // User activation does not survive a large read. Reserve an empty tab now, but
            // never let the final document retain an opener or send the child a referrer.
            let popup = opener
                .open_with_url_and_target("about:blank", "_blank")
                .map_err(|_| "Could not create a preview tab.")?
                .ok_or(
                    "The browser blocked the preview tab. Allow pop-ups for Corral and try again.",
                )?;
            let mut preview = Self {
                opener,
                popup,
                parts: Array::new(),
                slot: Some(slot),
                launched: false,
            };
            preview
                .popup
                .set_opener(&JsValue::NULL)
                .map_err(|_| "Could not isolate the preview tab.")?;
            if let Some(document) = preview.popup.document() {
                document.set_title("Preparing HTML snapshot…");
                if let Some(body) = document.body() {
                    body.set_text_content(Some("Preparing a sandboxed HTML snapshot. Switch back to Corral to see progress or cancel."));
                }
            }
            preview.push(PREFIX)?;
            Ok(preview)
        }

        fn blank(&self) -> bool {
            self.popup.closed() == Ok(false)
                && self.popup.location().href().ok().as_deref() == Some("about:blank")
        }

        fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
            let buffers = Array::new();
            buffers.push(&Uint8Array::from(bytes));
            // Compose from Blob parts rather than copying all their byte arrays at EOF.
            let part = Blob::new_with_u8_array_sequence(&buffers)
                .map_err(|_| "Could not stage the HTML snapshot.")?;
            self.parts.push(&part);
            Ok(())
        }

        fn launch(mut self) -> Result<(), String> {
            if !self.blank() {
                return Err("The preview tab was closed or left.".into());
            }
            self.push(SUFFIX)?;
            let props = BlobPropertyBag::new();
            props.set_type("text/html;charset=utf-8");
            let blob = Blob::new_with_blob_sequence_and_options(&self.parts, &props)
                .map_err(|_| "Could not create the HTML snapshot.")?;
            let url = Url::create_object_url_with_blob(&blob)
                .map_err(|_| "Could not create a browser URL for the HTML snapshot.")?;
            let retire = url.clone();
            let slot = self.slot.take();
            let revoke: Function = Closure::once_into_js(move || {
                let _ = Url::revoke_object_url(&retire);
                drop(slot);
            })
            .unchecked_into();
            let timer = match self
                .opener
                .set_timeout_with_callback_and_timeout_and_arguments_0(&revoke, 60_000)
            {
                Ok(timer) => timer,
                Err(_) => {
                    let _ = revoke.call0(&JsValue::UNDEFINED);
                    return Err("Could not schedule cleanup for the HTML snapshot.".into());
                }
            };
            if self.popup.location().replace(&url).is_err() {
                self.opener.clear_timeout_with_handle(timer);
                let _ = revoke.call0(&JsValue::UNDEFINED);
                return Err("Could not navigate the preview tab to its HTML snapshot.".into());
            }
            self.launched = true;
            Ok(())
        }
    }

    impl HtmlSink for WebPreview {
        fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
            if !self.blank() {
                return Err("The preview tab was closed or left.".into());
            }
            self.push(&escape(bytes))
        }

        fn finish(self: Box<Self>) -> HtmlFinish {
            Box::pin(async move { self.launch() })
        }
    }

    impl Drop for WebPreview {
        fn drop(&mut self) {
            if !self.launched && self.blank() {
                let _ = self.popup.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_cannot_escape_into_the_opened_tabs_origin() {
        let mut wrapped = PREFIX.to_vec();
        for piece in [b"\"></ifr".as_slice(), b"ame><script>bad()</script>&quot;"] {
            wrapped.extend(escape(piece));
        }
        wrapped.extend_from_slice(SUFFIX);
        let wrapped = String::from_utf8(wrapped).unwrap();
        assert!(wrapped.contains("sandbox=\"allow-scripts\""));
        assert!(!wrapped.contains("allow-same-origin"));
        assert_eq!(wrapped.matches("</iframe>").count(), 1);
        assert!(!wrapped.contains("<script>"));
        assert!(wrapped.contains("&amp;quot;"));
    }
}

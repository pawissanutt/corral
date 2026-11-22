//! Open a complete HTML snapshot in a separate sandboxed browser document.
use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use nsh_core::prelude::*;

use crate::client::{
    html::{OpenHtml, Output},
    requests::{self, ClientRequest},
};
use crate::model::{
    Id,
    daemon::Feed,
    session::{FilesView, files_large::HtmlProgress},
};
use crate::state::AppState;
use crate::ui::{
    kit::button::{ButtonKind, button},
    tokens::{Token, color},
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "files_native.rs"]
mod native;

/// A session's public id and a worktree path.
type File = (String, String);
/// Set once an open is abandoned; its sink then refuses to write or launch.
type Stop = Arc<AtomicBool>;

/// The Files tab's browser open, bound to the file it was started from.
#[derive(Clone, Copy)]
pub(super) struct Html {
    /// The Files tab's: an open outlives the redraws of the control that started it.
    owner: Owner,
    run: Signal<Option<Run>>,
    // The cleanup callback also holds this slot: signals may already be gone at disposal.
    retiring: Signal<Rc<std::cell::RefCell<Option<Run>>>>,
}

#[derive(Clone)]
struct Run {
    file: File,
    /// Written by the client, read only while `file` stays open.
    status: Signal<HtmlProgress>,
    stop: Stop,
}

impl PartialEq for Run {
    fn eq(&self, other: &Self) -> bool {
        self.status == other.status
    }
}

impl Run {
    /// Ends the transfer and refuses its launch, even one already waiting to finish.
    fn abandon(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(HtmlProgress::Reading { id, .. }) = self.status.try_get() {
            _ = requests::send(ClientRequest::CancelHtml(id));
        }
    }
}

impl Html {
    /// The status of `file`'s open; another file's never shows under it.
    fn status(self, file: &File) -> Option<Signal<HtmlProgress>> {
        self.run.with(|run| {
            run.as_ref()
                .filter(|run| run.file == *file)
                .map(|run| run.status)
        })
    }

    /// Ends any other open and records `file`'s.
    fn begin(self, file: File) -> Run {
        self.stop();
        let run = Run {
            file,
            status: self.owner.run(|| signal(HtmlProgress::Idle)),
            stop: Stop::default(),
        };
        self.retiring.get_untracked().replace(Some(run.clone()));
        self.run.set(Some(run.clone()));
        run
    }

    fn stop(self) {
        if let Some(run) = self.run.get_untracked() {
            self.retiring.get_untracked().take();
            self.run.set(None);
            run.abandon();
            // The client drops a transfer whose status no longer reads its id.
            run.status.try_set(HtmlProgress::Idle);
        }
    }
}

/// The open of the Files tab showing `shown`.
pub(super) fn progress(shown: Memo<Option<FilesView>>, beside: Memo<bool>) -> Html {
    bound(move || {
        shown.with(|files| {
            let files = files.as_ref()?;
            if !beside.get() && files.browsing {
                return None; // The narrow layout's Back button hid preview and Cancel.
            }
            let editor = files.editor.as_ref()?;
            Some((files.session.as_str().to_owned(), editor.path.clone()))
        })
    })
}

fn bound(open: impl Fn() -> Option<File> + 'static) -> Html {
    let retiring = Rc::new(std::cell::RefCell::new(None));
    let html = Html {
        owner: Owner::current(),
        run: signal(None),
        retiring: signal(retiring.clone()),
    };
    let open = computed(open);
    // Another file, Close or another session ends the open before it can launch; a redraw of
    // the same file keeps it.
    effect(move || {
        let open = open.get();
        untracked(|| {
            if html.run.with(|run| {
                run.as_ref()
                    .is_some_and(|run| Some(&run.file) != open.as_ref())
            }) {
                html.stop();
            }
        });
    });
    // Leaving the Files tab disposes it, and the status the client reads with it.
    cleanup(move || {
        if let Some(run) = retiring.take() {
            run.abandon();
        }
    });
    html
}

pub(super) fn browser_button(
    app: AppState,
    html: Html,
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
    let file: File = (session.as_str().to_owned(), path.to_owned());
    let source = source.filter(|_| !can_fetch).map(Rc::<str>::from);
    let progress = {
        let file = file.clone();
        computed(move || {
            html.status(&file)
                .map_or(HtmlProgress::Idle, |status| status.get())
        })
    };
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
                    html.stop();
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
                let (file, source) = (file.clone(), source.clone());
                controls = controls
                    .child(note("Opens a sandboxed copy. Inline scripts can run and may navigate the frame to external sites. Open only files you trust."))
                    .child(button(app, "Open in browser", ButtonKind::Ghost, "btn-open-html-browser", move || {
                        start(app, html, file.clone(), source.as_deref());
                    }));
            }
            Some(controls.into_element())
        }
    }
    }))
}

fn start(app: AppState, html: Html, file: File, source: Option<&str>) {
    if html.status(&file).is_some_and(|status| {
        matches!(
            status.get_untracked(),
            HtmlProgress::Reading { .. } | HtmlProgress::Launching
        )
    }) {
        return;
    }
    let Run { file, status, stop } = html.begin(file);
    let result = (|| {
        if !app.feed.with_untracked(Feed::ready) {
            return Err("The daemon is offline.".into());
        }
        let mut output = target(stop)?;
        if let Some(source) = source {
            output.append(source.as_bytes())?;
            status.set(HtmlProgress::Launching);
            // Not the button's: its redraw must not drop the snapshot mid-launch. Leaving the
            // file sets `stop`, which the sink checks before it launches.
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
                session: file.0,
                path: file.1,
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

#[cfg(any(target_arch = "wasm32", target_os = "linux", target_os = "macos"))]
const ABANDONED: &str = "The HTML open was cancelled.";

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn target(stop: Stop) -> Result<Output, String> {
    native::NativePreview::new(stop).map(|preview| Box::new(preview) as Output)
}

#[cfg(not(any(target_arch = "wasm32", target_os = "linux", target_os = "macos")))]
fn target(_: Stop) -> Result<Output, String> {
    Err("This app cannot launch an HTML preview on this device.".into())
}

#[cfg(target_arch = "wasm32")]
fn target(stop: Stop) -> Result<Output, String> {
    web::WebPreview::new(stop).map(|preview| Box::new(preview) as Output)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::atomic::Ordering;

    use js_sys::{Array, Function, Uint8Array};
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
    use web_sys::{Blob, BlobPropertyBag, Url, Window};

    use crate::client::html::{HtmlFinish, HtmlSink};

    use super::{ABANDONED, PREFIX, SUFFIX, Slot, Stop, escape};

    pub(super) struct WebPreview {
        opener: Window,
        popup: Window,
        parts: Array,
        slot: Option<Slot>,
        stop: Stop,
        launched: bool,
    }

    impl WebPreview {
        pub(super) fn new(stop: Stop) -> Result<Self, String> {
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
                stop,
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

        fn usable(&self) -> Result<(), String> {
            if self.stop.load(Ordering::Relaxed) {
                return Err(ABANDONED.into());
            }
            if !self.blank() {
                return Err("The preview tab was closed or left.".into());
            }
            Ok(())
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
            self.usable()?;
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
            self.usable()?;
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

    #[test]
    fn an_open_ends_with_its_file_but_survives_redraws() {
        use nsh_core::element::{Runtime, create_root};

        let runtime = Runtime::default();
        let _entered = runtime.enter();
        let file = |session: &str, path: &str| Some((session.to_owned(), path.to_owned()));
        // The open file, and a count of unrelated changes to the Files view.
        let (tab, (open, html)) = create_root(|| {
            let open = signal((file("B-1", "a.html"), 0));
            (open, bound(move || open.get().0))
        });
        let reading = |html: Html, at: Option<File>| {
            let run = html.begin(at.unwrap());
            run.status.set(HtmlProgress::Reading {
                id: "t".into(),
                percent: 40,
            });
            run
        };
        let current = || html.run.get_untracked().map(|run| run.status);
        let ended = |run: &Run| {
            run.stop.load(Ordering::Relaxed)
                && run.status.get_untracked() == HtmlProgress::Idle
                && current().is_none()
        };

        let first = reading(html, file("B-1", "a.html"));
        open.update(|(_, redraws)| *redraws += 1);
        assert_eq!(current(), Some(first.status), "a redraw keeps it");
        assert!(!first.stop.load(Ordering::Relaxed));
        assert_eq!(html.status(&first.file), Some(first.status));
        assert_eq!(html.status(&file("B-1", "b.html").unwrap()), None);

        for next in [file("B-1", "b.html"), None, file("B-2", "b.html")] {
            let run = reading(html, open.get_untracked().0.or(file("B-1", "b.html")));
            assert!(first.stop.load(Ordering::Relaxed), "one open at a time");
            open.update(|(shown, _)| *shown = next.clone());
            assert!(ended(&run), "another file, Close or session ends it");
        }

        let last = reading(html, open.get_untracked().0);
        tab.dispose();
        assert!(last.stop.load(Ordering::Relaxed), "leaving the tab ends it");
        assert_eq!(last.status.try_get(), None);
    }
}

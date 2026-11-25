//! A browser download. Where the browser offers its save picker (`showSaveFilePicker`), each
//! page is written to the chosen file's writable stream before the next is read, so no more than
//! a page waits in memory; the browser commits the file only on `close`, after the client has
//! verified size and SHA-256, and an abandoned download is aborted: a file that held something
//! keeps it, and the empty file the picker made for a new name is removed where the browser can
//! (`FileSystemHandle.remove`). Elsewhere
//! the bytes are staged as Blob parts, which the browser keeps in memory, so only artifacts up to
//! [`BLOB_MAX`] are offered that way.
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, HtmlElement, Url};

use crate::client::download::{SaveFinish, SaveOutput, SaveSink, SaveStep};

/// The largest download staged in browser memory when the browser has no save picker.
const BLOB_MAX: u64 = 64 * 1024 * 1024;

pub(super) async fn sink(name: &str, bytes: u64) -> Result<Option<SaveOutput>, String> {
    let window = web_sys::window().ok_or("No browser window is available.")?;
    let picker = Reflect::get(&window, &"showSaveFilePicker".into())
        .ok()
        .and_then(|picker| picker.dyn_into::<Function>().ok());
    if let Some(picker) = picker {
        return Picked::new(&window, &picker, name)
            .await
            .map(|picked| picked.map(|picked| Box::new(picked) as SaveOutput));
    }
    if bytes > BLOB_MAX {
        return Err(format!(
            "This browser holds a download in memory, so Corral saves at most {} here; use a browser with a save picker (Chrome or Edge) or the desktop app.",
            crate::model::session::transcript::size(BLOB_MAX)
        ));
    }
    Ok(Some(Box::new(Staged {
        name: name.to_owned(),
        parts: Array::new(),
        held: 0,
    })))
}

fn call(target: &JsValue, method: &str, args: &Array) -> Result<JsValue, JsValue> {
    let function: Function = Reflect::get(target, &method.into())?.dyn_into()?;
    function.apply(target, args)
}

async fn settle(value: Result<JsValue, JsValue>) -> Result<JsValue, JsValue> {
    JsFuture::from(value?.dyn_into::<Promise>()?).await
}

/// The file the owner picked, written as its pages arrive.
struct Picked {
    name: String,
    handle: JsValue,
    writable: JsValue,
    /// The picked file was empty, as the picker leaves a new name: removed if abandoned.
    empty: bool,
    closed: bool,
}

impl Picked {
    async fn new(
        window: &web_sys::Window,
        picker: &Function,
        name: &str,
    ) -> Result<Option<Self>, String> {
        let options = Object::new();
        let _ = Reflect::set(&options, &"suggestedName".into(), &name.into());
        let handle = match settle(picker.call1(window, &options)).await {
            Ok(handle) => handle,
            Err(error) => {
                let kind = Reflect::get(&error, &"name".into())
                    .ok()
                    .and_then(|kind| kind.as_string());
                // Cancelling the picker is not a failure.
                if kind.as_deref() == Some("AbortError") {
                    return Ok(None);
                }
                return Err("The browser could not open a file to save to.".into());
            }
        };
        let picked = Reflect::get(&handle, &"name".into())
            .ok()
            .and_then(|picked| picked.as_string())
            .unwrap_or_else(|| name.to_owned());
        let empty = match settle(call(&handle, "getFile", &Array::new())).await {
            Ok(file) => {
                Reflect::get(&file, &"size".into())
                    .ok()
                    .and_then(|size| size.as_f64())
                    == Some(0.0)
            }
            Err(_) => false,
        };
        let writable = settle(call(&handle, "createWritable", &Array::new()))
            .await
            .map_err(|_| "The browser could not write the chosen file.")?;
        Ok(Some(Self {
            name: picked,
            handle,
            writable,
            empty,
            closed: false,
        }))
    }
}

impl SaveSink for Picked {
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> SaveStep<'a> {
        Box::pin(async move {
            let chunk = Uint8Array::from(bytes);
            settle(call(&self.writable, "write", &Array::of1(&chunk)))
                .await
                .map(drop)
                .map_err(|_| "The browser could not write the chosen file.".to_owned())
        })
    }

    fn finish(mut self: Box<Self>) -> SaveFinish {
        Box::pin(async move {
            settle(call(&self.writable, "close", &Array::new()))
                .await
                .map_err(|_| "The browser could not finish the chosen file.".to_owned())?;
            // Only a committed file is kept; a failed close is abandoned on drop.
            self.closed = true;
            Ok(format!("as {}", self.name))
        })
    }
}

impl Drop for Picked {
    fn drop(&mut self) {
        // An unfinished stream is discarded: the chosen file keeps what it held before.
        if self.closed {
            return;
        }
        let (handle, writable, empty) = (self.handle.clone(), self.writable.clone(), self.empty);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = settle(call(&writable, "abort", &Array::new())).await;
            if empty {
                let _ = settle(call(&handle, "remove", &Array::new())).await;
            }
        });
    }
}

/// Blob parts the browser holds until the read is complete; then one browser download.
struct Staged {
    name: String,
    parts: Array,
    held: u64,
}

impl Staged {
    fn stage(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.held += bytes.len() as u64;
        if self.held > BLOB_MAX {
            return Err("The download grew past what this browser holds in memory.".into());
        }
        let part = Blob::new_with_u8_array_sequence(&Array::of1(&Uint8Array::from(bytes)))
            .map_err(|_| "Could not stage the download.")?;
        self.parts.push(&part);
        Ok(())
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
        // The browser reads the URL after the click returns, so it is retired a minute later;
        // without a timer it stays until the page goes.
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&revoke, 60_000);
        // A page is not told when the browser's download completes.
        Ok(format!(
            "as {}: handed to the browser's downloads",
            self.name
        ))
    }
}

impl SaveSink for Staged {
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> SaveStep<'a> {
        Box::pin(std::future::ready(self.stage(bytes)))
    }

    fn finish(self: Box<Self>) -> SaveFinish {
        Box::pin(async move { self.save() })
    }
}

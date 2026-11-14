use std::{cell::Cell, rc::Rc};

use corral_protocol::{device::NOISE_TAG, frame::MESSAGE_MAX};
use futures_channel::mpsc::UnboundedSender;
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{BinaryType, CloseEvent, MessageEvent, WebSocket};

use super::Input;

const MAX_QUEUED_MESSAGES: usize = 256;
const MAX_MESSAGE_BYTES: usize = MESSAGE_MAX + NOISE_TAG;
const CLIENT_CLOSE: u16 = 4000;

pub(super) struct Socket {
    ws: WebSocket,
    queued: Rc<Cell<usize>>,
    closing: Rc<Cell<bool>>,
    _onopen: Closure<dyn FnMut()>,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onclose: Closure<dyn FnMut(CloseEvent)>,
}

impl Socket {
    pub(super) fn open(
        port: u16,
        path: &str,
        generation: u64,
        tx: UnboundedSender<Input>,
    ) -> Result<Self, String> {
        // A path must not be able to extend or replace the loopback authority.
        if !path.starts_with('/') {
            return Err("the browser socket path must start with /".into());
        }
        let ws = WebSocket::new(&format!("ws://127.0.0.1:{port}{path}"))
            .map_err(|err| js_error("opening browser socket", err))?;
        ws.set_binary_type(BinaryType::Arraybuffer);
        let queued = Rc::new(Cell::new(0usize));
        let closing = Rc::new(Cell::new(false));

        let onopen = {
            let tx = tx.clone();
            let ws = ws.clone();
            let closing = Rc::clone(&closing);
            Closure::<dyn FnMut()>::new(move || {
                if !closing.get() && tx.unbounded_send(Input::Opened { generation }).is_err() {
                    closing.set(true);
                    let _ = ws.close();
                }
            })
        };
        let onmessage = {
            let tx = tx.clone();
            let ws = ws.clone();
            let queued = Rc::clone(&queued);
            let closing = Rc::clone(&closing);
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                if closing.get() {
                    return;
                }
                let Ok(buffer) = event.data().dyn_into::<ArrayBuffer>() else {
                    closing.set(true);
                    let _ = ws.close_with_code_and_reason(CLIENT_CLOSE, "protocol_violation");
                    return;
                };
                // Inspect the JS-owned buffer before allocating any Rust payload.
                if buffer.byte_length() as usize > MAX_MESSAGE_BYTES {
                    closing.set(true);
                    let _ = ws.close_with_code_and_reason(CLIENT_CLOSE, "protocol_violation");
                    return;
                }
                if queued.get() >= MAX_QUEUED_MESSAGES {
                    closing.set(true);
                    let _ = ws.close_with_code_and_reason(CLIENT_CLOSE, "client_backlog");
                    return;
                }
                let bytes = Uint8Array::new(buffer.as_ref()).to_vec();
                queued.set(queued.get() + 1);
                if tx
                    .unbounded_send(Input::Message { generation, bytes })
                    .is_err()
                {
                    queued.set(queued.get().saturating_sub(1));
                    closing.set(true);
                    let _ = ws.close();
                }
            })
        };
        let onclose = {
            let closing = Rc::clone(&closing);
            Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
                closing.set(true);
                let _ = tx.unbounded_send(Input::Closed {
                    generation,
                    code: event.code(),
                    reason: event.reason(),
                });
            })
        };
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        Ok(Self {
            ws,
            queued,
            closing,
            _onopen: onopen,
            _onmessage: onmessage,
            _onclose: onclose,
        })
    }

    pub(super) fn send(&self, bytes: &[u8]) -> Result<(), String> {
        if self.closing.get() || self.ws.ready_state() != WebSocket::OPEN {
            return Err("the browser socket is not open".into());
        }
        self.ws
            .send_with_u8_array(bytes)
            .map_err(|err| js_error("sending browser socket message", err))
    }

    pub(super) fn received(&self) {
        self.queued.set(self.queued.get().saturating_sub(1));
    }

    pub(super) fn close(&self) {
        self.closing.set(true);
        let _ = self.ws.close();
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        self.close();
    }
}

fn js_error(context: &str, error: JsValue) -> String {
    let detail = error.as_string().unwrap_or_else(|| format!("{error:?}"));
    format!("{context}: {detail}")
}

//! UI intents only; each transport accepts its own device-management subset.
#[derive(Debug)]
pub enum ClientRequest {
    Enrol { name: String },
    Revoke { device: String },
    PairBegin,
    PairConfirm { pair_id: String },
    PairCancel { pair_id: String },
    Pair { code: String, name: String },
    Forget,
}

#[cfg(not(target_arch = "wasm32"))]
static SENDER: std::sync::OnceLock<
    std::sync::Mutex<Option<std::sync::mpsc::Sender<ClientRequest>>>,
> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn install(sender: std::sync::mpsc::Sender<ClientRequest>) {
    *SENDER
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(sender);
}

pub fn send(request: ClientRequest) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        SENDER
            .get()
            .and_then(|sender| {
                sender
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_ref()
                    .map(|sender| sender.send(request).is_ok())
            })
            .unwrap_or(false)
    }
    #[cfg(target_arch = "wasm32")]
    {
        super::web::send(request)
    }
}

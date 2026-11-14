//! UI intents only; each transport accepts its own device-management subset.
use corral_protocol::attachment::AttachmentKind;
use corral_protocol::{app::*, assistant::*, cmd::CmdResultQuery, detail::DetailGet};

#[derive(Debug)]
pub struct AttachRequest {
    pub upload: String,
    pub kind: AttachmentKind,
    pub name: String,
    pub media_type: String,
    pub bytes: nsh_core::image::ImageData,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct AttachPath {
    pub upload: String,
    pub kind: AttachmentKind,
    pub path: String,
}

#[derive(Debug)]
pub enum ClientRequest {
    Enrol {
        name: String,
    },
    Revoke {
        device: String,
    },
    PairBegin,
    PairConfirm {
        pair_id: String,
    },
    PairCancel {
        pair_id: String,
    },
    Pair {
        code: String,
        name: String,
    },
    Forget,
    Detail {
        request_id: String,
        request: DetailGet,
    },
    ActPreview {
        request_id: String,
        request: ActPreviewRequest,
    },
    ActSubmit(ActSubmit),
    ActResult(CmdResultQuery),
    AssistantSend(AssistantSend),
    AssistantCancel(String),
    Attach(AttachRequest),
    AttachDiscard(String),
    #[cfg(not(target_arch = "wasm32"))]
    AttachPath(AttachPath),
    #[cfg(not(target_arch = "wasm32"))]
    AttachLoaded {
        upload: String,
        kind: AttachmentKind,
        name: String,
        result: Result<(Vec<u8>, String), crate::model::daemon::ClientError>,
    },
    AssistantWarm {
        request_id: String,
        request: AssistantWarm,
    },
    AssistantStop {
        request_id: String,
        request: AssistantStop,
    },
    AssistantSession {
        request_id: String,
        request: AssistantSessionOp,
    },
    PromptAnswer(PromptAnswer),
    ChangesPage {
        request_id: String,
        request: ChangesQuery,
    },
    PromptPreview {
        request_id: String,
        request: PromptPreview,
    },
    SeenAdvance {
        request_id: String,
        request: SeenAdvance,
    },
    DeviceSetAuthority {
        request_id: String,
        request: DeviceSetAuthority,
    },
    Thread {
        scope: String,
        open: bool,
    },
    SaveDraft {
        thread: String,
        text: String,
    },
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

/// Allocated once per user intent; reconnect recovery never replaces command identities.
pub fn new_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(target_arch = "wasm32"))]
    let random = {
        use std::io::Read;
        let mut bytes = [0u8; 16];
        let read =
            std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
        if read.is_ok() {
            u128::from_ne_bytes(bytes)
        } else {
            jiff::Timestamp::now().as_nanosecond() as u128 ^ u128::from(std::process::id())
        }
    };
    #[cfg(target_arch = "wasm32")]
    let random = {
        let mut bytes = [0u8; 16];
        let filled = web_sys::window()
            .and_then(|window| window.crypto().ok())
            .is_some_and(|crypto| crypto.get_random_values_with_u8_array(&mut bytes).is_ok());
        if filled {
            u128::from_ne_bytes(bytes)
        } else {
            (js_sys::Date::now().to_bits() as u128) << 64 | js_sys::Math::random().to_bits() as u128
        }
    };
    format!("app-{random:032x}-{sequence:x}")
}

macro_rules! operation {
    ($fn:ident, $ty:ty, $variant:ident) => {
        pub fn $fn(request: $ty) -> String {
            let request_id = new_id();
            send(ClientRequest::$variant {
                request_id: request_id.clone(),
                request,
            });
            request_id
        }
    };
}
operation!(detail, DetailGet, Detail);
operation!(act_preview, ActPreviewRequest, ActPreview);
operation!(prompt_preview, PromptPreview, PromptPreview);
operation!(assistant_warm, AssistantWarm, AssistantWarm);
operation!(assistant_stop, AssistantStop, AssistantStop);
operation!(assistant_session, AssistantSessionOp, AssistantSession);
operation!(changes_page, ChangesQuery, ChangesPage);
operation!(seen_advance, SeenAdvance, SeenAdvance);
operation!(device_set_authority, DeviceSetAuthority, DeviceSetAuthority);

pub fn act_submit(request: ActSubmit) -> bool {
    send(ClientRequest::ActSubmit(request))
}
pub fn act_result(request: CmdResultQuery) -> bool {
    send(ClientRequest::ActResult(request))
}
pub fn assistant_send(request: AssistantSend) -> bool {
    send(ClientRequest::AssistantSend(request))
}
pub fn prompt_answer(request: PromptAnswer) -> bool {
    send(ClientRequest::PromptAnswer(request))
}
pub fn thread(scope: String, open: bool) -> bool {
    send(ClientRequest::Thread { scope, open })
}
pub fn save_draft(thread: String, text: String) -> bool {
    send(ClientRequest::SaveDraft { thread, text })
}
pub fn attach(request: AttachRequest) -> bool {
    send(ClientRequest::Attach(request))
}
pub fn attach_discard(upload: String) -> bool {
    send(ClientRequest::AttachDiscard(upload))
}
pub fn assistant_cancel(cmd_id: String) -> bool {
    send(ClientRequest::AssistantCancel(cmd_id))
}
#[cfg(not(target_arch = "wasm32"))]
pub fn attach_path(request: AttachPath) -> bool {
    send(ClientRequest::AttachPath(request))
}

//! UI intents only; each transport accepts its own device-management subset.
use corral_protocol::attachment::AttachmentKind;
use corral_protocol::build::{BUILD_START, BuildStart};
use corral_protocol::config::ConfigDocument;
use corral_protocol::device::{DeviceRevoke, PushKind};
use corral_protocol::op::OpSubmit;
use corral_protocol::session_host::{SESSION_SEND, SessionSend};
use corral_protocol::{app::*, assistant::*, detail::DetailGet};

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
    DetailWatch {
        target: corral_protocol::detail::DetailRef,
        open: bool,
    },
    Detail {
        request_id: String,
        request: DetailGet,
    },
    SessionItem {
        session: String,
        token: String,
    },
    Watch {
        session: String,
        on: bool,
    },
    /// `op.submit`, answered on the connection; never journaled or replayed.
    OpSubmit {
        request_id: String,
        request: OpSubmit,
    },
    ConfigGet {
        request_id: String,
    },
    ConfigPut {
        request_id: String,
        request: ConfigDocument,
    },
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
    AssistantSession {
        request_id: String,
        request: AssistantSessionOp,
    },
    /// `prompt.answer`: an agent's question answered in place, read by `request_id`.
    PromptAnswer {
        request_id: String,
        request: corral_protocol::session::SessionPromptAnswer,
    },
    /// A session request addressed by the session's public id, read by `request_id`: a control
    /// (`session.cancel`, `.pause`, `.resume`, `.terminate`, `.relaunch`, `.rewind`), owner
    /// input (`session.send`) or a build start (`build.start`).
    Session {
        request_id: String,
        method: &'static str,
        params: serde_json::Value,
    },
    ChangesPage {
        request_id: String,
        request: ChangesQuery,
    },
    SeenAdvance {
        request_id: String,
        request: SeenAdvance,
    },
    DeviceRevoke {
        request_id: String,
        request: DeviceRevoke,
    },
    BrowserPush {
        request_id: String,
        enabled: bool,
        kinds: Vec<PushKind>,
    },
    PushStatus {
        request_id: String,
    },
    RetryTopic(String),
    Terminal {
        session: String,
        open: bool,
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
operation!(assistant_warm, AssistantWarm, AssistantWarm);
operation!(assistant_session, AssistantSessionOp, AssistantSession);
operation!(changes_page, ChangesQuery, ChangesPage);
operation!(seen_advance, SeenAdvance, SeenAdvance);
pub fn device_revoke(device: String) -> String {
    let request_id = new_id();
    send(ClientRequest::DeviceRevoke {
        request_id: request_id.clone(),
        request: DeviceRevoke { device },
    });
    request_id
}
pub fn terminal(session: String, open: bool) -> bool {
    send(ClientRequest::Terminal { session, open })
}
/// Sends the operation under `request_id`, the key its status is read by.
pub fn submit(request_id: String, request: OpSubmit) -> bool {
    send(ClientRequest::OpSubmit {
        request_id,
        request,
    })
}
pub fn assistant_send(request: AssistantSend) -> bool {
    send(ClientRequest::AssistantSend(request))
}
pub fn thread(scope: String, open: bool) -> bool {
    send(ClientRequest::Thread { scope, open })
}
pub fn session_item(session: String, token: String) -> bool {
    send(ClientRequest::SessionItem { session, token })
}
pub fn watch(session: String, on: bool) -> bool {
    send(ClientRequest::Watch { session, on })
}
/// Owner input to a session under `request_id`, which is also the send's command id and so the
/// input id the host and the transcript keep.
pub fn session_send(request_id: String, request: SessionSend) -> bool {
    send(ClientRequest::Session {
        request_id,
        method: SESSION_SEND,
        params: serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
    })
}
/// A build session's start under `request_id`; the started row or the host's refusal is read
/// under it.
pub fn build_start(request_id: String, request: BuildStart) -> bool {
    send(ClientRequest::Session {
        request_id,
        method: BUILD_START,
        params: serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
    })
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

/// Full jitter in the upper half of a capped exponential reconnect window.
pub(super) fn reconnect_delay_ms(failures: usize) -> u64 {
    let cap = match failures {
        0 => 1_000,
        1 => 2_000,
        2 => 5_000,
        _ => 10_000,
    };
    let entropy = new_id().bytes().fold(0u64, |seed, byte| {
        seed.wrapping_mul(31).wrapping_add(u64::from(byte))
    });
    cap / 2 + entropy % (cap / 2 + 1)
}

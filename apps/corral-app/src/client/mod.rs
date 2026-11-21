//! Optional Unix enrollment and authenticated WebSocket/Noise connections.
mod app;
mod core;
#[cfg(unix)]
mod credentials;
#[cfg(unix)]
mod driver;
mod durable;
#[cfg(not(target_arch = "wasm32"))]
mod durable_native;
#[cfg(target_arch = "wasm32")]
mod durable_web;
mod map;
mod offline;
mod outbox;
#[cfg(any(test, target_arch = "wasm32", windows))]
mod pairing_link;
pub mod requests;
#[cfg(unix)]
mod session;
#[cfg(unix)]
mod socket;
#[cfg(unix)]
mod transport;
#[cfg(any(target_arch = "wasm32", windows))]
mod web;
#[cfg(test)]
#[path = "web/source_tests.rs"]
mod web_source_tests;

pub const PAIRED_KEY: &str = "corral.paired";

pub fn pairing_supported() -> bool {
    cfg!(any(target_arch = "wasm32", windows))
}

/// Read an explicit browser pairing link, clearing its secret fragment before using it.
/// This only prepares the form: pairing still requires the user's Pair action.
pub fn pairing_link() -> Result<Option<String>, &'static str> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(None)
    }
    #[cfg(target_arch = "wasm32")]
    {
        pairing_link::read()
    }
}

/// Whether there is something to connect to: a Unix socket or a saved pairing.
pub fn configured() -> bool {
    #[cfg(unix)]
    {
        std::env::var_os("CORRAL_SOCKET").is_some_and(|value| !value.is_empty())
    }
    #[cfg(any(target_arch = "wasm32", windows))]
    {
        saved_pairing(nsh_core::storage::Storage::get_item(PAIRED_KEY).as_deref())
    }
}

/// The NSH Storage marker hints that there is a pairing to resume. Confirmed pairing
/// writes it; Forget, revocation and missing credentials remove it. The saved credential
/// is authoritative: one found without the marker is still resumed and restores the marker.
pub fn saved_pairing(marker: Option<&str>) -> bool {
    marker.is_some_and(|marker| !marker.is_empty())
}

/// Start once under the app owner; load authenticated cached data before dialing.
pub fn start(feed: nsh_core::prelude::Signal<crate::model::daemon::Feed>) {
    #[cfg(unix)]
    driver::start(
        feed,
        socket::resolve(std::env::var_os("CORRAL_SOCKET"))
            .unwrap_or_else(|| Err("No daemon socket configured.".into())),
    );
    #[cfg(any(target_arch = "wasm32", windows))]
    web::start(feed);
}

pub fn push_supported() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Reflect::has(
            &js_sys::global(),
            &wasm_bindgen::JsValue::from_str("PushManager"),
        )
        .unwrap_or(false)
    }
}

pub fn page_origin() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window().and_then(|window| window.location().origin().ok())
    }
}

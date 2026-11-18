//! Optional enrolled native and browser daemon connections.
mod app;
mod core;
#[cfg(not(target_arch = "wasm32"))]
mod credentials;
#[cfg(not(target_arch = "wasm32"))]
mod driver;
mod durable;
#[cfg(not(target_arch = "wasm32"))]
mod durable_native;
#[cfg(target_arch = "wasm32")]
mod durable_web;
mod map;
mod offline;
mod outbox;
#[cfg(any(test, target_arch = "wasm32"))]
mod pairing_link;
pub mod requests;
#[cfg(not(target_arch = "wasm32"))]
mod session;
#[cfg(not(target_arch = "wasm32"))]
mod socket;
#[cfg(all(not(target_arch = "wasm32"), unix))]
mod transport;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(test)]
#[path = "web/source_tests.rs"]
mod web_source_tests;

pub const PAIRED_KEY: &str = "corral.paired";

pub fn pairing_supported() -> bool {
    cfg!(target_arch = "wasm32")
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

/// Whether there is something to connect to: a native socket, or a browser's saved pairing.
pub fn configured() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os("CORRAL_SOCKET").is_some_and(|value| !value.is_empty())
    }
    #[cfg(target_arch = "wasm32")]
    {
        saved_pairing(nsh_core::storage::Storage::get_item(PAIRED_KEY).as_deref())
    }
}

/// A browser has a pairing to resume when NSH Storage holds the paired marker, which a
/// confirmed pairing writes and Forget, revocation or a missing credential remove. Without it
/// the browser opens on the pairing form at once rather than dialing nothing. The credential in
/// IndexedDB stays the authority: one found without the marker is still resumed, and the marker
/// is written again.
pub fn saved_pairing(marker: Option<&str>) -> bool {
    marker.is_some_and(|marker| !marker.is_empty())
}

/// Start once under the app owner; load authenticated cached data before dialing.
pub fn start(feed: nsh_core::prelude::Signal<crate::model::daemon::Feed>) {
    #[cfg(not(target_arch = "wasm32"))]
    driver::start(
        feed,
        socket::resolve(std::env::var_os("CORRAL_SOCKET"))
            .unwrap_or_else(|| Err("No daemon socket configured.".into())),
    );
    #[cfg(target_arch = "wasm32")]
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

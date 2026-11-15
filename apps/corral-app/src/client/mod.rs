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

/// Whether a native endpoint exists, or browser credential probing is supported.
pub fn configured() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os("CORRAL_SOCKET").is_some_and(|value| !value.is_empty())
    }
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
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

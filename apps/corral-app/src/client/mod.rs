//! Optional enrolled native and browser daemon connections.
mod app;
mod core;
#[cfg(not(target_arch = "wasm32"))]
mod credentials;
#[cfg(not(target_arch = "wasm32"))]
mod driver;
mod map;
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

/// Whether a native endpoint or a persisted browser pairing is configured.
pub fn configured() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os("CORRAL_SOCKET").is_some_and(|value| !value.is_empty())
    }
    #[cfg(target_arch = "wasm32")]
    {
        nsh_core::storage::Storage::get_item(PAIRED_KEY).as_deref() == Some("1")
    }
}

/// Start once under the app owner; no task, thread or I/O when unconfigured.
pub fn start(feed: nsh_core::prelude::Signal<crate::model::daemon::Feed>) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(socket) = socket::resolve(std::env::var_os("CORRAL_SOCKET")) {
        driver::start(feed, socket);
    }
    #[cfg(target_arch = "wasm32")]
    web::start(feed);
}

//! Optional native daemon connection; fixtures do not start any socket work.
#[cfg(not(target_arch = "wasm32"))]
mod driver;
#[cfg(not(target_arch = "wasm32"))]
mod map;
#[cfg(not(target_arch = "wasm32"))]
mod session;
#[cfg(not(target_arch = "wasm32"))]
mod socket;
#[cfg(all(not(target_arch = "wasm32"), unix))]
mod transport;

/// Whether CORRAL_SOCKET is nonempty. Always false on wasm.
pub fn configured() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os("CORRAL_SOCKET").is_some_and(|value| !value.is_empty())
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Start once under the app owner; no task, thread or I/O when unconfigured.
pub fn start(feed: nsh_core::prelude::Signal<crate::model::daemon::Feed>) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(socket) = socket::resolve(std::env::var_os("CORRAL_SOCKET")) {
        driver::start(feed, socket);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = feed;
}

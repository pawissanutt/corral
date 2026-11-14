#[cfg(unix)]
#[path = "session_unix.rs"]
mod native;
#[cfg(unix)]
pub(super) use native::Session;

#[cfg(not(unix))]
pub(super) struct Session;
#[cfg(not(unix))]
impl Session {
    pub(super) fn connect(
        _socket: &std::path::Path,
    ) -> Result<(Self, corral_protocol::Welcome), String> {
        Err("daemon peer credentials are unavailable on this platform".into())
    }

    pub(super) fn open(
        &mut self,
        _topic: corral_protocol::subscription::Topic,
    ) -> Result<(), String> {
        Err("daemon sockets are unavailable on this platform".into())
    }

    pub(super) fn request(
        &mut self,
        _method: &'static str,
        _params: serde_json::Value,
    ) -> Result<(), String> {
        Err("daemon sockets are unavailable on this platform".into())
    }

    pub(super) fn next(&mut self) -> crate::client::core::Event {
        crate::client::core::Event::Closed("daemon sockets are unavailable on this platform".into())
    }
}

#[cfg(all(test, unix))]
#[path = "session_tests.rs"]
mod tests;

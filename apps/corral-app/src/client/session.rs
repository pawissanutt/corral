use corral_protocol::Health;
use corral_protocol::board::BoardRow;

pub(super) enum Event {
    Health(Health),
    Board {
        epoch: u64,
        rows: Vec<BoardRow>,
        left: Vec<String>,
        replacement: bool,
    },
    Idle,
    Closed(String),
}

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

    pub(super) fn next(&mut self) -> Event {
        Event::Closed("daemon sockets are unavailable on this platform".into())
    }
}

#[cfg(all(test, unix))]
#[path = "session_tests.rs"]
mod tests;

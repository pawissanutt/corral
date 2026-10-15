//! Socket identity follows herdr's selectors, independently of its config-file override.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::Herdr;

impl Herdr {
    /// Stable identity of the server targeted by this client. Explicit sessions override
    /// inherited sockets; the named "default" session uses the unnamed server's socket.
    pub fn socket_path(&self) -> Result<PathBuf> {
        socket_path(self.session(), |name| std::env::var_os(name))
    }
}

fn socket_path(session: Option<&str>, env: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    let nonempty = |name| env(name).filter(|value| !value.is_empty());
    if session.is_none()
        && let Some(socket) = nonempty("HERDR_SOCKET_PATH")
    {
        return Ok(socket.into());
    }
    let inherited = nonempty("HERDR_SESSION");
    let session = session
        .map(OsString::from)
        .or(inherited)
        .unwrap_or_else(|| "default".into());
    let base = if let Some(xdg) = nonempty("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("herdr")
    } else {
        PathBuf::from(nonempty("HOME").context("locating herdr socket: HOME is not set")?)
            .join(".config/herdr")
    };
    Ok(if session == "default" {
        base.join("herdr.sock")
    } else {
        base.join("sessions").join(session).join("herdr.sock")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_identity_matches_engine_selector_precedence() {
        // Given explicit and inherited selectors, without mutating process globals.
        for (session, socket, inherited, xdg, expected) in [
            (None, Some("/socket"), Some("test"), None, "/socket"),
            (
                Some("test"),
                Some("/socket"),
                None,
                None,
                "/home/test/.config/herdr/sessions/test/herdr.sock",
            ),
            (
                Some("default"),
                Some("/socket"),
                Some("test"),
                None,
                "/home/test/.config/herdr/herdr.sock",
            ),
            (
                None,
                None,
                Some("test"),
                Some("/xdg"),
                "/xdg/herdr/sessions/test/herdr.sock",
            ),
            (None, None, None, Some("/xdg"), "/xdg/herdr/herdr.sock"),
        ] {
            // When resolving the target socket.
            let actual = socket_path(session, |name| match name {
                "HOME" => Some("/home/test".into()),
                "XDG_CONFIG_HOME" => xdg.map(Into::into),
                "HERDR_SOCKET_PATH" => socket.map(Into::into),
                "HERDR_SESSION" => inherited.map(Into::into),
                _ => None,
            })
            .unwrap();
            // Then aliases and overrides identify the same server as herdr.
            assert_eq!(actual, PathBuf::from(expected));
        }
    }
}

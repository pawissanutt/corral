//! Where herdr reads its `config.toml` (keybindings, prefix). Shared by the plugin setup that writes
//! corral's keys and the board that names them.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result};

/// Resolves herdr's explicit override, XDG directory, then the native config directory.
pub fn config_path() -> Result<PathBuf> {
    resolve_path(|name| std::env::var_os(name))
}

fn resolve_path(env: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    let nonempty = |name| env(name).filter(|value| !value.is_empty());
    if let Some(path) = nonempty("HERDR_CONFIG_PATH") {
        return Ok(path.into());
    }
    if let Some(path) = nonempty("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("herdr/config.toml"));
    }
    #[cfg(windows)]
    {
        if let Some(path) = nonempty("APPDATA") {
            return Ok(PathBuf::from(path).join("herdr/config.toml"));
        }
        if let Some(path) = nonempty("USERPROFILE") {
            return Ok(PathBuf::from(path).join("AppData/Roaming/herdr/config.toml"));
        }
    }
    let home = nonempty("HOME").context("locating herdr config: HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/herdr/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn native_windows_config_uses_appdata_before_profile_and_home() {
        // Given native environment paths and no explicit/XDG override.
        for (appdata, expected) in [
            (Some(r"C:\config"), r"C:\config\herdr\config.toml"),
            (None, r"C:\users\test\AppData\Roaming\herdr\config.toml"),
        ] {
            // When resolving the same precedence as the engine.
            let path = resolve_path(|name| match name {
                "APPDATA" => appdata.map(Into::into),
                "USERPROFILE" => Some(r"C:\users\test".into()),
                "HOME" => Some(r"C:\other".into()),
                _ => None,
            })
            .unwrap();
            // Then plugin setup targets the native config, not HOME/.config.
            assert_eq!(path, PathBuf::from(expected));
        }
    }

    #[test]
    fn config_paths_follow_override_xdg_home_precedence() {
        // Given isolated environment values, without changing process globals.
        for (values, expected) in [
            (
                [Some("/explicit.toml"), Some("/xdg"), Some("/home/test")],
                "/explicit.toml",
            ),
            (
                [None, Some("/xdg"), Some("/home/test")],
                "/xdg/herdr/config.toml",
            ),
            (
                [Some(""), Some(""), Some("/home/test")],
                "/home/test/.config/herdr/config.toml",
            ),
        ] {
            // When resolving a path.
            let actual = resolve_path(|name| match name {
                "HERDR_CONFIG_PATH" => values[0].map(Into::into),
                "XDG_CONFIG_HOME" => values[1].map(Into::into),
                "HOME" => values[2].map(Into::into),
                _ => None,
            })
            .unwrap();
            // Then only the highest priority nonempty setting wins.
            assert_eq!(actual, PathBuf::from(expected));
        }
        assert!(resolve_path(|_| None).is_err());
    }
}

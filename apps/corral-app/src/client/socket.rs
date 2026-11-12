use std::ffi::OsString;
use std::path::PathBuf;

pub(super) fn resolve(value: Option<OsString>) -> Option<Result<PathBuf, String>> {
    let value = value.filter(|value| !value.is_empty())?;
    let path = PathBuf::from(value);
    Some(
        if path.is_absolute() && !path.as_os_str().as_encoded_bytes().contains(&0) {
            Ok(path)
        } else {
            Err("CORRAL_SOCKET must be an absolute path".into())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_socket_is_configured_but_rejected_without_discovery() {
        assert!(resolve(None).is_none());
        assert!(resolve(Some(OsString::new())).is_none());
        assert!(resolve(Some("auto".into())).is_some_and(|path| path.is_err()));
        assert!(resolve(Some("/tmp/corrald.sock".into())).is_some_and(|path| path.is_ok()));
    }
}

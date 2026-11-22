use anyhow::{Context, Result};
use corral_herdr::Herdr;
use corral_protocol::pane::PaneKeys;

pub(super) fn send(herdr: &Herdr, pane: &str, request: &PaneKeys) -> Result<()> {
    if let Some(text) = request.text.as_deref() {
        herdr
            .pane_send_text(pane, text, false)
            .and_then(|delivery| match delivery.error {
                Some(error) => Err(anyhow::Error::msg(error)),
                None => Ok(()),
            })
            .with_context(|| format!("send text to herdr pane {pane}"))?;
        if request.keys.is_empty() {
            return Ok(());
        }
    }
    let keys: Vec<_> = request.keys.iter().map(String::as_str).collect();
    herdr
        .pane_send_keys(pane, &keys)
        .with_context(|| format!("send keys to herdr pane {pane}"))
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;

    #[test]
    fn text_only_does_not_submit_and_explicit_keys_follow_text() {
        let root = tempfile::Builder::new()
            .prefix(".corral-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let bin = root.path().join("herdr");
        fs::write(
            &bin,
            r#"#!/bin/sh
set -eu
cd "${0%/*}"
case "$1 $2" in
  'pane read')
    printf '> '
    if [ -f composer ]; then cat composer; fi
    printf '\n'
    ;;
  'pane send-text')
    printf '%s\0' "$@" >> calls
    printf '%s' "$4" > composer
    ;;
  'pane send-keys') printf '%s\0' "$@" >> calls ;;
  *) exit 1 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        // A test thread forking mid-write briefly holds the script open for writing, and running
        // it then fails with "text file busy". Probe without arguments (no call is recorded).
        while matches!(
            std::process::Command::new(&bin).output(),
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy
        ) {
            std::thread::yield_now();
        }
        let herdr = Herdr::with_bin(bin, None).with_poll_wait(|_| {});
        let calls = root.path().join("calls");
        let pane = "w1:p1";
        let request = |text, keys: Option<&[&str]>| -> PaneKeys {
            let mut params = serde_json::json!({"task":"T-001","run":"R1","text":text});
            if let Some(keys) = keys {
                params["keys"] = serde_json::json!(keys);
            }
            serde_json::from_value(params).unwrap()
        };
        let keys = Some(["tab", "enter"].as_slice());
        assert!(send(&herdr, pane, &request("", keys)).is_err());
        assert!(!calls.exists());

        send(&herdr, pane, &request("hello $USER; &", None)).unwrap();
        assert_eq!(
            fs::read(&calls).unwrap().as_slice(),
            b"pane\0send-text\0w1:p1\0hello $USER; &\0"
        );
        fs::remove_file(&calls).unwrap();

        send(&herdr, pane, &request("next", keys)).unwrap();
        assert_eq!(
            fs::read(&calls).unwrap().as_slice(),
            b"pane\0send-text\0w1:p1\0next\0pane\0send-keys\0w1:p1\0tab\0enter\0"
        );
    }
}

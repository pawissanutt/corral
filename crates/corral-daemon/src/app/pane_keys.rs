use anyhow::{Context, Result};
use corral_herdr::Herdr;

pub(super) fn send(herdr: &Herdr, pane: &str, text: Option<&str>, keys: &[String]) -> Result<()> {
    if let Some(text) = text {
        herdr
            .pane_send_text(pane, text, false)
            .and_then(|delivery| match delivery.error {
                Some(error) => Err(anyhow::Error::msg(error)),
                None => Ok(()),
            })
            .with_context(|| format!("send text to herdr pane {pane}"))?;
        if keys.is_empty() {
            return Ok(());
        }
    }
    let keys: Vec<_> = keys.iter().map(String::as_str).collect();
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
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
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
        let herdr = Herdr::with_bin(bin, None).with_poll_wait(|_| {});
        let calls = root.path().join("calls");
        let pane = "w1:p1";
        let keys = ["tab".to_owned(), "enter".to_owned()];

        assert!(send(&herdr, pane, Some(""), &keys).is_err());
        assert!(!calls.exists());

        send(&herdr, pane, Some("hello $USER; &"), &[]).unwrap();
        assert_eq!(
            fs::read(&calls).unwrap().as_slice(),
            b"pane\0send-text\0w1:p1\0hello $USER; &\0"
        );
        fs::remove_file(&calls).unwrap();

        send(&herdr, pane, Some("next"), &keys).unwrap();
        assert_eq!(
            fs::read(&calls).unwrap().as_slice(),
            b"pane\0send-text\0w1:p1\0next\0pane\0send-keys\0w1:p1\0tab\0enter\0"
        );
    }
}

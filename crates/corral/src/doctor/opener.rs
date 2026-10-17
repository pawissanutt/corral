//! The browser `corral artifact serve` (and opening an HTML artifact) reaches from here.

use std::collections::BTreeMap;

use corral_ops::viewers::{Host, URL_VIEWER, UrlRoute, program_available, url_route};

use super::Check;

const FIX: &str = "Set `[viewers] url` (e.g. `corral config set viewers.url '\"firefox {url}\"'`) or CORRAL_OPENER to an installed browser command, or unset it to use the default opener.";

/// Fails (never hard) when the selected launcher is missing or a configured opener cannot be parsed. A printed
/// URL over SSH or without a display is a supported route, not a failure.
pub(super) fn artifact_opener(viewers: &BTreeMap<String, String>, host: &dyn Host) -> Check {
    let configured = if viewers.contains_key(URL_VIEWER) {
        Some("[viewers] url")
    } else {
        host.env("CORRAL_OPENER").map(|_| "CORRAL_OPENER")
    };
    let (ok, message) = match url_route(viewers, "http://127.0.0.1:<port>/", 0, host) {
        UrlRoute::Launch(argv) if program_available(&argv[0], host) => {
            (true, format!("served artifacts open with {}", argv[0]))
        }
        UrlRoute::Launch(argv) => (
            false,
            format!(
                "served artifacts would open with {}, which is not installed or not executable{}",
                argv[0],
                configured.map_or(String::new(), |c| format!(" (from {c})"))
            ),
        ),
        UrlRoute::Print(why) => match configured {
            Some(source) => (false, format!("{source} cannot be used: {why}")),
            None => (
                true,
                format!(
                    "served artifact URLs are printed ({why}); set [viewers] url or CORRAL_OPENER to choose a browser"
                ),
            ),
        },
    };
    Check {
        id: "artifact_opener",
        ok,
        hard: false,
        message,
        fix: (!ok).then(|| FIX.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use anyhow::{Result, bail};

    use super::*;

    #[derive(Default)]
    struct FakeHost {
        path: Vec<&'static str>,
        env: Vec<(&'static str, String)>,
    }

    impl Host for FakeHost {
        fn os(&self) -> &str {
            "linux"
        }
        fn proc_version(&self) -> Option<String> {
            Some("Linux version 6.8.0-generic".into())
        }
        fn on_path(&self, program: &str) -> bool {
            self.path.contains(&program)
        }
        fn env(&self, key: &str) -> Option<String> {
            self.env
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        }
        fn output(&self, argv: &[String]) -> Result<String> {
            bail!("doctor must not run {argv:?}")
        }
        fn launch(&self, argv: &[String], _: bool, _: bool) -> Result<()> {
            bail!("doctor must not launch {argv:?}")
        }
    }

    fn host(path: &[&'static str], env: &[(&'static str, &str)]) -> FakeHost {
        FakeHost {
            path: path.to_vec(),
            env: env.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        }
    }

    fn viewers(url: Option<&str>) -> BTreeMap<String, String> {
        url.map(|u| (URL_VIEWER.to_string(), u.to_string()))
            .into_iter()
            .collect()
    }

    #[test]
    fn a_missing_configured_opener_fails_softly_with_a_fix() {
        let check = artifact_opener(
            &viewers(None),
            &host(
                &[],
                &[("CORRAL_OPENER", "/definitely/missing/t069-browser")],
            ),
        );
        assert!(!check.ok && !check.hard, "{check:?}");
        assert!(
            check.message.contains("/definitely/missing/t069-browser")
                && check.message.contains("CORRAL_OPENER"),
            "{check:?}"
        );
        assert!(check.fix.is_some());

        let check = artifact_opener(&viewers(Some("nosuchbrowser {url}")), &host(&[], &[]));
        assert!(!check.ok && !check.hard, "{check:?}");
        assert!(check.message.contains("[viewers] url"), "{check:?}");
    }

    #[test]
    fn a_malformed_configured_opener_fails_softly() {
        for (viewer, env) in [
            (Some("firefox \"{url}"), None),
            (Some("   "), None),
            (None, Some("browser 'unterminated")),
        ] {
            let env: Vec<_> = env.map(|e| ("CORRAL_OPENER", e)).into_iter().collect();
            let check = artifact_opener(&viewers(viewer), &host(&["firefox", "browser"], &env));
            assert!(!check.ok && !check.hard, "{viewer:?} {env:?}: {check:?}");
            assert!(check.message.contains("cannot be used"), "{check:?}");
        }
    }

    #[test]
    fn available_openers_and_printed_urls_are_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("my-browser");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exe = exe.to_str().unwrap();
        let cases = [
            (viewers(None), host(&[], &[("CORRAL_OPENER", exe)])),
            (viewers(Some("firefox --new-tab")), host(&["firefox"], &[])),
            (viewers(None), host(&["xdg-open"], &[("DISPLAY", ":0")])),
            // SSH and headless hosts print the URL (with a port-forward hint): a supported route.
            (
                viewers(None),
                host(&[], &[("SSH_CONNECTION", "10.0.0.2 5000 10.0.0.1 22")]),
            ),
            (viewers(None), host(&[], &[])),
        ];
        for (v, h) in cases {
            let check = artifact_opener(&v, &h);
            assert!(check.ok && check.fix.is_none(), "{check:?}");
        }
        let ssh = artifact_opener(
            &viewers(None),
            &host(&[], &[("SSH_CONNECTION", "10.0.0.2 5000 10.0.0.1 22")]),
        );
        assert!(ssh.message.contains("ssh -N -L"), "{ssh:?}");
    }

    #[test]
    fn a_display_without_xdg_open_prints_the_url_and_names_the_fix() {
        let check = artifact_opener(&viewers(None), &host(&[], &[("DISPLAY", ":0")]));
        assert!(check.ok, "{check:?}");
        assert!(check.message.contains("xdg-open not found"), "{check:?}");
    }
}

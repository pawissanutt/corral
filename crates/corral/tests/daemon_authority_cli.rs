//! Daemon lifecycle controls are owner-only even outside assistant entry.
use std::process::{Command, Stdio};

#[test]
fn pm_and_agent_markers_cannot_start_stop_or_install_but_can_read_health_and_nudge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let store = corral_core::Store::init(&root).unwrap();
    let before = store.events().unwrap().len();
    for marker in ["CORRAL_PM", "AI_AGENT", "PI_CODING_AGENT"] {
        let command = || {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
            cmd.env_clear()
                .env(marker, "1")
                .env("HOME", &home)
                .env("PATH", &home)
                .env("XDG_CONFIG_HOME", home.join("config"))
                .env("XDG_DATA_HOME", home.join("data"))
                .env("XDG_STATE_HOME", home.join("state"))
                .env("XDG_CACHE_HOME", home.join("cache"))
                .env("XDG_RUNTIME_DIR", home.join("runtime"))
                .env("HERDR_BIN_PATH", home.join("missing-herdr"))
                .arg("--root")
                .arg(&root)
                .arg("daemon")
                .current_dir(&root)
                .stdin(Stdio::null());
            cmd
        };
        for args in [
            ["run", "--once"].as_slice(),
            &["stop", "--drain"],
            &["install"],
        ] {
            let output = command().args(args).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(1),
                "{marker} {args:?}: {output:?}"
            );
            assert!(output.stdout.is_empty(), "{marker} {args:?}: {output:?}");
            assert!(!home.join("runtime").exists(), "{marker} started a daemon");
            assert!(
                !home.join(".config/systemd").exists(),
                "{marker} installed a unit"
            );
            assert!(
                !home.join("Library/LaunchAgents").exists(),
                "{marker} installed a launch agent"
            );
        }
        let health = command().args(["status", "--json"]).output().unwrap();
        assert_eq!(health.status.code(), Some(2), "{marker}: {health:?}");
        let health: serde_json::Value = serde_json::from_slice(&health.stdout).unwrap();
        assert_eq!(health["daemon"], "down");
        let nudge = command().arg("nudge").output().unwrap();
        assert_eq!(nudge.status.code(), Some(2), "{marker}: {nudge:?}");
        assert!(
            !nudge.stdout.is_empty(),
            "{marker} was refused before nudging"
        );
    }
    assert_eq!(store.events().unwrap().len(), before);
}

#[test]
fn socket_discovery_matches_listener_path_without_a_running_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    let expected = corral_ops::daemon_client::socket_path(&store);
    assert!(expected.is_absolute());
    assert!(!expected.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_PM", "1")
        .arg("daemon")
        .arg("--root")
        .arg(dir.path())
        .arg("socket")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", expected.display())
    );
    assert!(!expected.exists());
}

//! Subprocess acceptance tests with isolated HOME, PATH and a fake engine.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

struct Sandbox {
    _tmp: TempDir,
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

const ENGINE: &str = r#"#!/bin/sh
dir=${0%/*}
if [ "$1" = "--version" ]; then
  printf 'herdr %s\n' "${FAKE_VERSION:-0.9.4}"
  exit 0
fi
printf '%s\n' "$*" >> "$dir/calls"
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "plugin list")
    if [ -e "$dir/linked" ]; then
      printf '{"result":{"plugins":[{"plugin_id":"corral","enabled":true,"manifest_path":"%s/.corral/state/herdr-plugin/herdr-plugin.toml"}]}}\n' "$FAKE_ROOT"
    else printf '%s\n' '{"result":{"plugins":[]}}'; fi ;;
  "plugin link") : > "$dir/linked"; printf '%s\n' '{"result":{}}' ;;
  "server ") : > "$dir/running" ;;
  "workspace list")
    if [ ! -e "$dir/running" ]; then
      printf '%s\n' '{"error":{"code":"server_not_running","message":"raw transport failure"}}' >&2
      exit 1
    fi
    if [ "${FAKE_EXISTING:-}" = 1 ]; then
      printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"existing"}]}}'
    else printf '%s\n' '{"result":{"workspaces":[]}}'; fi ;;
  "pane list")
    if [ -e "$dir/board" ]; then
      printf '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","title":"corral board","cwd":"%s"}]}}\n' "$FAKE_ROOT"
    else printf '%s\n' '{"result":{"panes":[]}}'; fi ;;
  "workspace create")
    printf '%s\n' '{"result":{"root_pane":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1"}}}' ;;
  "pane run") : > "$dir/board"; printf '%s\n' '{"result":{}}' ;;
  "pane get")
    printf '%s\n' '{"result":{"pane":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1"}}}' ;;
  *) printf '%s\n' '{"result":{}}' ;;
esac
"#;

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace");
        let home = tmp.path().join("home");
        let bin = tmp.path().join("bin");
        for dir in [&root, &home, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // Config's detector recognizes a git checkout without needing any git subprocess.
        std::fs::create_dir(root.join(".git")).unwrap();
        corral_core::Store::init(&root).unwrap();
        for (name, text) in [
            ("herdr", ENGINE),
            ("claude", "#!/bin/sh\nprintf 'claude 1.0\\n'\n"),
        ] {
            let file = bin.join(name);
            std::fs::write(&file, text).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            _tmp: tmp,
            root,
            home,
            bin,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for key in [
            "HERDR_ENV",
            "HERDR_SOCKET_PATH",
            "HERDR_PANE_ID",
            "HERDR_WORKSPACE_ID",
            "HERDR_TAB_ID",
            "HERDR_CONFIG_PATH",
            "CORRAL_ROOT",
            "CORRAL_HERDR_SESSION",
        ] {
            cmd.env_remove(key);
        }
        cmd.arg("--root")
            .arg(&self.root)
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("PATH", &self.bin)
            .env("HERDR_BIN_PATH", self.bin.join("herdr"))
            .env("FAKE_ROOT", &self.root)
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn touch(&self, name: &str) {
        std::fs::write(self.bin.join(name), "").unwrap();
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.bin.join("calls")).unwrap()
    }

    fn repair(&self) {
        self.touch("running");
        let out = self.run(&["doctor", "--fix"]);
        assert!(out.status.success(), "{}", text(&out));
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn report(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(out)))
}

fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap()
}

#[test]
fn doctor_reports_existing_done_and_dropped_worktree_directories_only() {
    let s = Sandbox::new();
    let store = corral_core::Store::open(&s.root).unwrap();
    for (status, name, exists) in [
        (corral_core::Status::Done, "done", true),
        (corral_core::Status::Dropped, "dropped", true),
        (corral_core::Status::Running, "running", true),
        (corral_core::Status::Done, "missing", false),
    ] {
        let path = s.root.join(".worktrees").join(name);
        if exists {
            std::fs::create_dir_all(&path).unwrap();
        }
        let task = store.create(name, None, status, "").unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(serde_json::from_value(serde_json::json!({
                "id":"R1", "agent":"generic", "worktree":path, "branch":format!("corral/{name}"),
                "base":"main", "base_sha":"abc", "started":"2026-10-15T00:00:00Z"
            }))?);
                Ok(())
            })
            .unwrap();
    }
    let out = s.run(&["doctor", "--json"]);
    let report = report(&out);
    let check = check(&report, "terminal_worktrees");
    assert_eq!(check["ok"], false);
    let message = check["message"].as_str().unwrap();
    assert!(message.contains("/done"));
    assert!(message.contains("/dropped"));
    assert!(!message.contains("/running"));
    assert!(!message.contains("/missing"));
}

#[test]
fn discard_help_exposes_keep_branch_option() {
    let s = Sandbox::new();
    let out = s.run(&["discard", "--help"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("--keep-branch"));
}

#[test]
fn doctor_reports_install_command_when_herdr_absent_from_path() {
    // Given a PATH with only an agent harness.
    let s = Sandbox::new();
    std::fs::remove_file(s.bin.join("herdr")).unwrap();
    // When doctor runs without an explicit binary override.
    let out = s
        .command(&["doctor", "--json"])
        .env_remove("HERDR_BIN_PATH")
        .output()
        .unwrap();
    // Then the failure identifies the engine install and preserves pure JSON output.
    assert!(!out.status.success());
    let value = report(&out);
    assert_eq!(check(&value, "herdr_version")["ok"], false);
    assert!(
        check(&value, "herdr_version")["fix"]
            .as_str()
            .unwrap()
            .contains("https://herdr.dev/install.sh")
    );
}

#[test]
fn doctor_reports_minimum_version_when_engine_too_old() {
    // Given an engine below the plugin's minimum.
    let s = Sandbox::new();
    // When its version is checked.
    let out = s
        .command(&["doctor", "--json"])
        .env("FAKE_VERSION", "0.9.2")
        .output()
        .unwrap();
    // Then the required and found versions and exact update command are available.
    let value = report(&out);
    let version = check(&value, "herdr_version");
    assert_eq!(version["ok"], false);
    assert!(version["message"].as_str().unwrap().contains("0.9.3"));
    assert!(version["message"].as_str().unwrap().contains("0.9.2"));
    assert!(
        version["fix"]
            .as_str()
            .unwrap()
            .contains("https://herdr.dev/install.sh")
    );
}

#[test]
fn doctor_reports_up_when_server_is_down() {
    // Given a working executable with no server.
    let s = Sandbox::new();
    // When doctor checks the session.
    let out = s.run(&["doctor", "--json"]);
    // Then the recovery is corral up, not a raw transport error.
    let value = report(&out);
    assert_eq!(check(&value, "herdr_server")["ok"], false);
    assert!(
        check(&value, "herdr_server")["fix"]
            .as_str()
            .unwrap()
            .contains("corral up")
    );
    assert!(!text(&out).contains("raw transport failure"));
}

#[test]
fn doctor_reports_plugin_install_when_unlinked() {
    // Given an unlinked plugin.
    let s = Sandbox::new();
    s.touch("running");
    // When checked without --fix.
    let value = report(&s.run(&["doctor", "--json"]));
    // Then the actionable fix names the plugin install command.
    assert_eq!(check(&value, "plugin")["ok"], false);
    assert!(
        check(&value, "plugin")["fix"]
            .as_str()
            .unwrap()
            .contains("corral plugin install")
    );
    assert!(!s.bin.join("linked").exists());
}

#[test]
fn doctor_reports_safe_merge_when_keybindings_missing() {
    // Given no engine configuration.
    let s = Sandbox::new();
    // When checked without mutation.
    let value = report(&s.run(&["doctor", "--json"]));
    // Then keybindings have their own repair and no config was created.
    assert_eq!(check(&value, "keybindings")["ok"], false);
    assert!(
        check(&value, "keybindings")["fix"]
            .as_str()
            .unwrap()
            .contains("corral doctor --fix")
    );
    assert!(!s.home.join(".config/herdr/config.toml").exists());
}

#[test]
fn doctor_fix_backs_up_merges_and_is_idempotent() {
    // Given a user config with unrelated content.
    let s = Sandbox::new();
    s.touch("running");
    let dir = s.home.join(".config/herdr");
    std::fs::create_dir_all(&dir).unwrap();
    let original = "# personal settings\n[theme]\nname = \"nord\"\n";
    std::fs::write(dir.join("config.toml"), original).unwrap();
    // When repaired twice, through the JSON interface.
    let first = s.run(&["doctor", "--fix", "--json"]);
    let second = s.run(&["doctor", "--fix", "--json"]);
    // Then the original has exactly one backup, the merge preserves it, and every check passes.
    assert!(first.status.success(), "{}", text(&first));
    assert!(second.status.success(), "{}", text(&second));
    let backups: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains(".bak"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(dir.join("config.toml")).unwrap()).unwrap();
    assert_eq!(config["theme"]["name"].as_str(), Some("nord"));
    assert_eq!(config["keys"]["command"].as_array().unwrap().len(), 4);
    assert!(
        report(&second)["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["ok"] == true)
    );
    assert!(s.home.join(".claude/skills/corral/SKILL.md").is_file());
    assert!(!s.calls().contains("pane "));
    assert!(!s.calls().contains("agent "));
}

#[test]
fn up_starts_isolated_session_with_first_board_pane_outside_herdr() {
    // Given an initialized workspace and no engine server.
    let s = Sandbox::new();
    // When up targets an explicitly isolated session.
    let out = s.run(&["up", "--session", "corral-test-up"]);
    // Then it bootstraps and runs the board in its first pane without touching other sessions.
    assert!(out.status.success(), "{}", text(&out));
    let calls = s.calls();
    assert!(calls.contains("--session corral-test-up server"), "{calls}");
    assert!(
        calls.contains("--session corral-test-up pane run w1:p1"),
        "{calls}"
    );
    assert!(calls.contains(" board"), "{calls}");
    assert!(
        calls
            .lines()
            .all(|line| line.starts_with("--session corral-test-up ")),
        "{calls}"
    );
}

#[test]
fn up_opens_board_without_nested_attach_inside_herdr() {
    // Given a running engine with an existing workspace.
    let s = Sandbox::new();
    s.touch("running");
    // When up runs inside a pane.
    let out = s
        .command(&["up"])
        .env("HERDR_ENV", "1")
        .env("FAKE_EXISTING", "1")
        .output()
        .unwrap();
    // Then the board opens through the plugin in that engine, with no server launch or attach.
    assert!(out.status.success(), "{}", text(&out));
    let calls = s.calls();
    assert!(
        calls.contains("plugin pane open --plugin corral --entrypoint board"),
        "{calls}"
    );
    assert!(!calls.contains("--session"), "{calls}");
    assert!(!calls.lines().any(|line| line == "server"), "{calls}");
}

#[test]
fn bare_corral_routes_to_the_same_up_behavior() {
    // Given repaired setup and a board that up can focus.
    let s = Sandbox::new();
    s.repair();
    s.touch("board");
    // When invoked bare and then explicitly as up.
    std::fs::write(s.bin.join("calls"), "").unwrap();
    let bare = s.run(&[]);
    let bare_calls = s.calls();
    std::fs::write(s.bin.join("calls"), "").unwrap();
    let up = s.run(&["up"]);
    // Then routing and engine effects are identical.
    assert!(bare.status.success(), "{}", text(&bare));
    assert!(up.status.success(), "{}", text(&up));
    assert_eq!(bare_calls, s.calls());
}

#[test]
fn up_stops_with_fix_list_when_engine_too_old() {
    // Given an old executable.
    let s = Sandbox::new();
    // When startup checks fail.
    let out = s
        .command(&["up"])
        .env("FAKE_VERSION", "0.8.0")
        .output()
        .unwrap();
    // Then no session/pane is created and the installation command is shown.
    assert!(!out.status.success());
    assert!(text(&out).contains("https://herdr.dev/install.sh"));
    assert!(!s.calls().contains("pane "));
    assert!(!s.calls().contains("server\n"));
}

#[test]
fn up_offers_detected_repositories_before_initialization() {
    // Given an uninitialized workspace with a detected repository.
    let s = Sandbox::new();
    std::fs::remove_dir_all(s.root.join(".corral")).unwrap();
    // When started non-interactively without consent.
    let out = s.run(&["up"]);
    // Then it offers init without silently creating workspace files.
    assert!(!out.status.success());
    assert!(text(&out).contains("corral init"));
    assert!(text(&out).contains("workspace"));
    assert!(!s.root.join(".corral").exists());
}

#[test]
fn up_yes_initializes_detected_repositories() {
    // Given an uninitialized repository.
    let s = Sandbox::new();
    std::fs::remove_dir_all(s.root.join(".corral")).unwrap();
    // When initialization is explicitly accepted.
    let out = s.run(&["up", "--yes"]);
    // Then the board session and workspace are ready.
    assert!(out.status.success(), "{}", text(&out));
    assert!(s.root.join(".corral/corral.toml").is_file());
    assert!(s.bin.join("board").is_file());
}

#[test]
fn board_uses_friendly_engine_error_before_entering_terminal_mode() {
    // Given an unavailable engine.
    let s = Sandbox::new();
    // When opening the board directly.
    let out = s.run(&["board"]);
    // Then recovery is shared with doctor and no raw transport error reaches the CLI.
    assert!(!out.status.success());
    assert!(text(&out).contains(corral_herdr::ENGINE_DOWN));
    assert!(!text(&out).contains("raw transport failure"));
}

#[test]
fn dispatch_uses_friendly_engine_error_when_server_is_down() {
    // Given a task in a real repository but no server.
    let s = Sandbox::new();
    std::fs::remove_dir(s.root.join(".git")).unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=t@t",
            "commit",
            "--allow-empty",
            "-qm",
            "init",
        ],
    ] {
        let out = Command::new("git")
            .arg("-C")
            .arg(&s.root)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out));
    }
    let out = s.run(&["new", "Test dispatch"]);
    assert!(out.status.success(), "{}", text(&out));
    // When dispatch reaches the engine boundary.
    let out = s.run(&["dispatch", "T-001", "--profile", "claude"]);
    // Then it gives the same actionable engine error as board and doctor.
    assert!(!out.status.success());
    assert!(
        text(&out).contains(corral_herdr::ENGINE_DOWN),
        "{}",
        text(&out)
    );
    assert!(!text(&out).contains("raw transport failure"));
}

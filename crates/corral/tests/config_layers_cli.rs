use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
    xdg: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let home = temp.path().join("home");
        let xdg = temp.path().join("xdg");
        for path in [&root, &home, &xdg] {
            std::fs::create_dir_all(path).unwrap();
        }
        let fixture = Self {
            _temp: temp,
            root,
            home,
            xdg,
        };
        fixture.ok(&["init"]);
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr")
            .arg("--root")
            .arg(&self.root)
            .current_dir(&self.root)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
        text(&output)
    }

    fn workspace(&self) -> PathBuf {
        self.root.join(".corral/corral.toml")
    }

    fn user(&self) -> PathBuf {
        self.xdg.join("corral/config.toml")
    }

    fn write_user(&self, text: &str) {
        write(&self.user(), text);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn explicit_empty_arrays_have_effective_values_and_workspace_sources() {
    let fixture = Fixture::new();
    fixture.write_user(
        "# personal arguments\n[profiles.omo]\nargs = [\"--personal\"]\n[harnesses.omo]\nefforts = [\"low\"]\n",
    );
    let personal = std::fs::read_to_string(fixture.user()).unwrap();
    for key in ["profiles.omo.args", "harnesses.omo.efforts"] {
        fixture.ok(&["config", "set", key, "[]", "--workspace"]);
        let sourced = fixture.ok(&["config", "get", key, "--source"]);
        assert_eq!(sourced.lines().next(), Some("[]"));
        assert!(
            sourced.contains(&format!("workspace ({})", fixture.workspace().display())),
            "{sourced}"
        );
        assert_eq!(fixture.ok(&["config", "get", key]).trim(), "[]");
    }
    assert_eq!(std::fs::read_to_string(fixture.user()).unwrap(), personal);
}

#[test]
fn xdg_user_values_load_and_sources_include_files_and_defaults() {
    let fixture = Fixture::new();
    fixture.write_user("[ui]\nmouse = false\ndone_max = 23\n");
    write(&fixture.workspace(), "[ui]\ndone_max = 19\n");
    let user = fixture.ok(&["config", "get", "ui.mouse", "--source"]);
    assert!(user.starts_with("false\n"), "{user}");
    assert!(
        user.contains(&format!("user ({})", fixture.user().display())),
        "{user}"
    );
    let workspace = fixture.ok(&["config", "get", "ui.done_max", "--source"]);
    assert!(workspace.starts_with("19\n"), "{workspace}");
    assert!(
        workspace.contains(&format!("workspace ({})", fixture.workspace().display())),
        "{workspace}"
    );
    let default = fixture.ok(&["config", "get", "limits.running", "--source"]);
    assert!(default.starts_with("3\n"), "{default}");
    assert!(default.contains("source: default"), "{default}");
}

#[test]
fn absent_or_empty_xdg_uses_home_config_without_reading_other_xdg_files() {
    let fixture = Fixture::new();
    write(&fixture.workspace(), "");
    fixture.write_user("[ui]\nmouse = true\n");
    let home_config = fixture.home.join(".config/corral/config.toml");
    write(&home_config, "[ui]\nmouse = false\n");
    for empty in [false, true] {
        let mut command = fixture.command();
        if empty {
            command.env("XDG_CONFIG_HOME", "");
        } else {
            command.env_remove("XDG_CONFIG_HOME");
        }
        let output = command
            .args(["config", "get", "ui.mouse", "--source"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        assert!(text(&output).starts_with("false\n"));
        assert!(text(&output).contains(&home_config.display().to_string()));
    }
}

#[test]
fn targeted_writes_preserve_comments_and_do_not_copy_inherited_values() {
    let fixture = Fixture::new();
    fixture.write_user("# personal notes\n[ui]\nmouse = false # preference\n");
    let original = "# project notes\n[agents.omo]\ncommand = 'cat'\n";
    write(&fixture.workspace(), original);
    fixture.ok(&["config", "set", "ui.mouse", "true", "--user"]);
    assert_eq!(
        std::fs::read_to_string(fixture.user()).unwrap(),
        "# personal notes\n[ui]\nmouse = true # preference\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace()).unwrap(),
        original
    );
    fixture.ok(&["config", "set", "ui.done_max", "29"]);
    fixture.ok(&["config", "set", "ui.done_max", "31", "--workspace"]);
    let workspace = std::fs::read_to_string(fixture.workspace()).unwrap();
    assert!(workspace.contains("# project notes\n"), "{workspace}");
    assert!(workspace.contains("done_max = 31"), "{workspace}");
    assert!(!workspace.contains("mouse"), "{workspace}");
}

#[test]
fn workspace_override_is_effective_after_masked_user_write() {
    let fixture = Fixture::new();
    fixture.ok(&["config", "set", "ui.mouse", "false", "--workspace"]);
    let output = fixture.ok(&["config", "set", "ui.mouse", "true", "--user"]);
    assert!(output.contains("effective source: workspace"), "{output}");
    assert_eq!(fixture.ok(&["config", "get", "ui.mouse"]).trim(), "false");
    fixture.ok(&["config", "check"]);
}

#[test]
fn user_writes_reject_all_nonpersonal_sections_without_changing_files() {
    let fixture = Fixture::new();
    fixture.write_user("# personal notes\n");
    let original = std::fs::read_to_string(fixture.workspace()).unwrap();
    for (key, value) in [
        ("projects.app.path", "."),
        ("review.max_rounds", "3"),
        ("limits.running", "7"),
        ("resources.allow_heavy", "true"),
    ] {
        let output = fixture.run(&["config", "set", key, value, "--user"]);
        assert!(!output.status.success(), "{key}");
        assert!(
            text(&output).contains("not allowed in the user layer"),
            "{}",
            text(&output)
        );
        assert!(text(&output).contains(&fixture.user().display().to_string()));
        assert_eq!(
            std::fs::read_to_string(fixture.user()).unwrap(),
            "# personal notes\n"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.workspace()).unwrap(),
            original
        );
    }
}

#[test]
fn check_reports_both_broken_files_even_without_opening_the_store() {
    let fixture = Fixture::new();
    fixture.write_user("[ui\n");
    write(&fixture.workspace(), "[limits\n");
    let output = fixture.run(&["config", "check"]);
    assert!(!output.status.success());
    let text = text(&output);
    assert!(
        text.contains(&fixture.user().display().to_string()),
        "{text}"
    );
    assert!(
        text.contains(&fixture.workspace().display().to_string()),
        "{text}"
    );
}

#[test]
fn check_reports_simultaneous_schema_errors_with_both_paths_and_keys() {
    // Given well-formed TOML with an invalid schema value in each file.
    let fixture = Fixture::new();
    fixture.write_user("[ui]\nmouse = 'wrong-user-type'\n");
    write(
        &fixture.workspace(),
        "[limits]\nrunning = 'wrong-workspace-type'\n",
    );
    // When the real CLI checks both inputs.
    let output = fixture.run(&["config", "check"]);
    // Then neither file's schema error hides the other.
    assert!(!output.status.success());
    let text = text(&output);
    for (path, key) in [
        (fixture.user(), "ui.mouse"),
        (fixture.workspace(), "limits.running"),
    ] {
        assert_eq!(
            text.matches(&path.display().to_string()).count(),
            1,
            "{text}"
        );
        assert!(text.contains(&format!("in `{key}`")), "{text}");
    }
}

#[test]
fn canonical_get_attributes_a_legacy_project_alias_to_workspace() {
    // Given a valid legacy config with a nondefault project profile and comments.
    let fixture = Fixture::new();
    let original = "# legacy project\n[projects.app]\npath = '.'\ndefault_agent = 'custom' # keep\n[agents.omo]\ncommand = 'cat'\n[agents.custom]\ncommand = 'cat'\n";
    write(&fixture.workspace(), original);
    fixture.ok(&["config", "check"]);
    // When the canonical key is queried through the actual CLI.
    let output = fixture.ok(&["config", "get", "projects.app.default_profile", "--source"]);
    // Then value and filename identify the explicit workspace input, not a built-in.
    assert_eq!(output.lines().next(), Some("custom"));
    assert!(
        output.contains(&format!("workspace ({})", fixture.workspace().display())),
        "{output}"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace()).unwrap(),
        original
    );
}

#[test]
fn set_user_and_workspace_flags_are_mutually_exclusive() {
    let fixture = Fixture::new();
    let output = fixture.run(&[
        "config",
        "set",
        "ui.mouse",
        "false",
        "--user",
        "--workspace",
    ]);
    assert!(!output.status.success());
}

#[test]
fn board_defaults_are_gettable_with_default_or_explicit_workspace_source() {
    let fixture = Fixture::new();
    let default = fixture.ok(&["config", "get", "board.card.column_cost", "--source"]);
    assert!(default.starts_with("false\n"), "{default}");
    assert!(default.contains("source: default"), "{default}");
    fixture.ok(&["config", "set", "board.card.column_cost", "false"]);
    let explicit = fixture.ok(&["config", "get", "board.card.column_cost", "--source"]);
    assert!(explicit.starts_with("false\n"), "{explicit}");
    assert!(explicit.contains("source: workspace"), "{explicit}");
}

#[test]
fn user_reloads_are_observed_once_and_keep_secrets_redacted() {
    const CHILD: &str = "CORRAL_LAYER_RELOAD_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let fixture = Fixture::new();
        // Run the real Store in a process with isolated HOME/XDG; never mutate test-global env.
        let mut command = Command::new(std::env::current_exe().unwrap());
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        let output = command
            .env(CHILD, "1")
            .env("HOME", &fixture.home)
            .env("XDG_CONFIG_HOME", &fixture.xdg)
            .args([
                "--exact",
                "user_reloads_are_observed_once_and_keep_secrets_redacted",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let mut board = corral_core::Store::init(root.path()).unwrap();
    let mut worker = board.clone();
    let path = corral_core::ConfigLayers::user_path().unwrap();
    write(
        &path,
        "[profiles.personal]\nharness = 'omo'\nargs = ['secret-one']\n",
    );
    assert!(board.reload_config_if_changed().unwrap());
    assert!(worker.reload_config_if_changed().unwrap());
    assert_eq!(board.config.profiles["personal"].args, ["secret-one"]);
    assert!(!worker.reload_config_if_changed().unwrap());
    write(
        &path,
        "[profiles.personal]\nharness = 'omo'\nargs = ['secret-two']\n",
    );
    assert!(board.reload_config_if_changed().unwrap());
    let good = board.config.clone();
    write(&path, "[ui]\nmouse = 'secret-invalid'\n");
    assert!(!board.reload_config_if_changed().unwrap());
    assert_eq!(board.config, good);
    std::fs::remove_file(&path).unwrap();
    assert!(board.reload_config_if_changed().unwrap());
    assert!(!board.config.profiles.contains_key("personal"));
    let events = board.events().unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "config_changed").count(),
        3
    );
    assert_eq!(
        events.iter().filter(|e| e.kind == "config_invalid").count(),
        1
    );
    for path in [
        board.dir().join("events.jsonl"),
        board.state_dir().join("config-observation.json"),
    ] {
        let text = std::fs::read_to_string(path).unwrap();
        for secret in ["secret-one", "secret-two", "secret-invalid"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

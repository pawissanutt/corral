//! Real config-check diagnostics, with private HOME/XDG paths and null stdin.
use std::process::{Command, Stdio};

fn check(user: &str, workspace: &str) -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    let config_home = root.path().join("config");
    let user_path = config_home.join("corral/config.toml");
    let workspace_path = root.path().join(".corral/corral.toml");
    std::fs::create_dir_all(user_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(workspace_path.parent().unwrap()).unwrap();
    std::fs::write(&user_path, user).unwrap();
    std::fs::write(&workspace_path, workspace).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    let output = command
        .arg("--root")
        .arg(root.path())
        .args(["config", "check"])
        .env("CORRAL_ROOT", root.path())
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", config_home)
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .current_dir(root.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    eprintln!("config check exit {:?}\n{stderr}", output.status.code());
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    (root, stderr)
}

#[test]
fn config_check_reports_cross_phase_failures_in_both_layer_directions() {
    // Given both reviewer reproductions and their layer-reversed equivalents.
    for (user, workspace, user_key, workspace_key) in [
        (
            "[ui\n",
            "[limits]\nrunning = 'wrong-workspace-type'\n[agents.omo]\ncommand = 'cat'\n",
            "[ui",
            "limits.running",
        ),
        (
            "[ui]\nmouse = 'wrong-user-type'\n",
            "[limits]\nrunning = 0\n[agents.omo]\ncommand = 'cat'\n",
            "ui.mouse",
            "limits.running",
        ),
        (
            "[ui]\nmouse = 'wrong-user-type'\n",
            "[limits\n",
            "ui.mouse",
            "[limits",
        ),
        (
            "[ui]\ndone_window = 'invalid-age'\n",
            "[limits]\nrunning = 'wrong-workspace-type'\n[agents.omo]\ncommand = 'cat'\n",
            "ui.done_window",
            "limits.running",
        ),
    ] {
        // When the candidate binary checks the isolated workspace.
        let (root, stderr) = check(user, workspace);
        // Then both filenames and offending keys are visible.
        for (path, key) in [
            ("config/corral/config.toml", user_key),
            (".corral/corral.toml", workspace_key),
        ] {
            assert!(
                stderr.contains(&root.path().join(path).display().to_string()),
                "{stderr}"
            );
            assert!(stderr.contains(key), "{stderr}");
        }
    }
}

#[test]
fn config_check_attributes_shared_and_masked_profile_errors_to_effort_only() {
    // Given the reviewer's split profile, with optional masking and literal dynamic names.
    for name in ["omo", "literal.profile", "literal effort"] {
        for masked in [false, true] {
            let user = format!("[profiles.\"{name}\"]\neffort = 'not-an-effort'\n");
            let workspace = format!(
                "[agents.omo]\ncommand = 'cat'\n\
                 [harnesses.omo]\ncommand = 'omo {{model_args}}'\nflags = 'omo'\n\
                 [profiles.\"{name}\"]\nharness = 'omo'\n{}",
                if masked { "effort = 'high'\n" } else { "" }
            );
            // When the candidate binary checks both effective and masked values.
            let (root, stderr) = check(&user, &workspace);
            // Then exactly one diagnostic names the personal effort, never the valid workspace.
            assert!(stderr.contains("invalid config (1 problem)"), "{stderr}");
            assert!(
                stderr.contains(&format!(
                    "{}: profiles.{name}.effort:",
                    root.path().join("config/corral/config.toml").display()
                )),
                "{stderr}"
            );
            assert!(
                !stderr.contains(
                    &root
                        .path()
                        .join(".corral/corral.toml")
                        .display()
                        .to_string()
                ),
                "{stderr}"
            );
        }
    }
}

#[test]
fn config_check_keeps_provable_workspace_errors_after_a_user_policy_error() {
    // Given the two policy-plus-profile fixtures from the independent review.
    for (workspace, key) in [
        (
            "[profiles.omo]\nharness = 'missing'\n",
            "profiles.omo.harness",
        ),
        ("[profiles.omo]\nmodel = 'custom'\n", "profiles.omo"),
    ] {
        // When the real candidate checks both parsed files.
        let (root, stderr) = check("[limits]\nrunning = 3\n", workspace);
        // Then the policy diagnostic does not hide the genuine workspace error.
        assert!(stderr.contains("invalid config (2 problems)"), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "{}: limits",
                root.path().join("config/corral/config.toml").display()
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains(
                &root
                    .path()
                    .join(".corral/corral.toml")
                    .display()
                    .to_string()
            ),
            "{stderr}"
        );
        assert!(stderr.contains(key), "{stderr}");
        assert!(stderr.contains("harness"), "{stderr}");
    }
}

#[test]
fn config_check_rejected_profiles_do_not_blame_valid_defaults_in_either_layer() {
    // Given a valid default referencing a schema-invalid declaration in either layer.
    for reverse in [false, true] {
        let bad = "[profiles.omo]\nharness = false\n";
        let good = "[defaults]\nprofile = 'omo'\n";
        let (user, workspace, bad_path, good_path) = if reverse {
            (
                bad,
                good,
                "config/corral/config.toml",
                ".corral/corral.toml",
            )
        } else {
            (
                good,
                bad,
                ".corral/corral.toml",
                "config/corral/config.toml",
            )
        };
        // When the real candidate recovers enough input to continue validation.
        let (root, stderr) = check(user, workspace);
        // Then only the actual schema-invalid field's file appears.
        assert!(stderr.contains("invalid config (1 problem)"), "{stderr}");
        assert!(
            stderr.contains(&root.path().join(bad_path).display().to_string()),
            "{stderr}"
        );
        assert!(stderr.contains("profiles.omo.harness"), "{stderr}");
        assert!(
            !stderr.contains(&root.path().join(good_path).display().to_string()),
            "{stderr}"
        );
    }
}

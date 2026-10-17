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

#[test]
fn config_check_retains_flags_after_required_command_schema_failure() {
    // Given the reviewer's invalid required field and independent other-layer flags.
    for reverse in [false, true] {
        let flags = "[harnesses.omo]\nflags = 'bogus'\n";
        let command = "[harnesses.omo]\ncommand = false\n";
        let (user, workspace, flags_path, command_path) = if reverse {
            (
                command,
                flags,
                ".corral/corral.toml",
                "config/corral/config.toml",
            )
        } else {
            (
                flags,
                command,
                "config/corral/config.toml",
                ".corral/corral.toml",
            )
        };
        // When the actual binary checks the isolated inputs.
        let (root, stderr) = check(user, &format!("{workspace}[agents.omo]\ncommand = 'cat'\n"));
        // Then both actual fields and files appear without an invented required-field error.
        assert!(stderr.contains("invalid config (2 problems)"), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "{}: harnesses.omo.flags:",
                root.path().join(flags_path).display()
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains(&root.path().join(command_path).display().to_string()),
            "{stderr}"
        );
        assert!(stderr.contains("harnesses.omo.command"), "{stderr}");
        assert!(!stderr.contains("missing field"), "{stderr}");
    }
}

#[test]
fn config_check_syntax_recovery_does_not_blame_valid_profile_references() {
    // Given a valid default and an unreadable profile inventory in either layer.
    for reverse in [false, true] {
        let bad = "[profiles.omo\n";
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
        // When the actual binary validates the readable layer.
        let (root, stderr) = check(user, workspace);
        // Then only the unreadable file is blamed.
        assert!(stderr.contains("invalid config (1 problem)"), "{stderr}");
        assert!(
            stderr.contains(&root.path().join(bad_path).display().to_string()),
            "{stderr}"
        );
        assert!(
            !stderr.contains(&root.path().join(good_path).display().to_string()),
            "{stderr}"
        );
    }
}

#[test]
fn config_check_preserves_recovery_dependencies_from_independent_review() {
    // Given the review's required-profile, inherited-capability, and reviewer dependencies.
    for (bad, other, workspace_only, count, bad_key, sibling_key) in [
        (
            "[profiles.omo]\nharness = false\n",
            "[profiles.omo]\nescalate_to = 'truly-missing'\n",
            "",
            2,
            "profiles.omo.harness",
            Some("profiles.omo.escalate_to"),
        ),
        (
            "[harnesses.custom\n",
            "[harnesses.custom]\ncommand = 'runner {model_args}'\n\
             [profiles.omo]\nharness = 'custom'\nmodel = 'custom-model'\n",
            "",
            1,
            "[harnesses.custom",
            None,
        ),
        (
            "[harnesses.custom]\nflags = false\n",
            "[harnesses.custom]\ncommand = 'runner {model_args}'\n\
             [profiles.omo]\nharness = 'custom'\nmodel = 'custom-model'\n",
            "",
            1,
            "harnesses.custom.flags",
            None,
        ),
        (
            "[harnesses.custom]\ncommand = false\n",
            "[profiles.omo]\nharness = 'custom'\nvendor = 'openai'\n",
            "[routing.reviewer_by_vendor]\nopenai = 'omo'\n",
            1,
            "harnesses.custom.command",
            None,
        ),
    ] {
        for reverse in [false, true] {
            let (user, workspace, bad_path, other_path) = if reverse {
                (
                    other,
                    bad,
                    ".corral/corral.toml",
                    "config/corral/config.toml",
                )
            } else {
                (
                    bad,
                    other,
                    "config/corral/config.toml",
                    ".corral/corral.toml",
                )
            };
            let workspace = format!("{workspace}[agents.omo]\ncommand = 'cat'\n{workspace_only}");
            // When the actual CLI checks both isolated layers.
            let (root, stderr) = check(user, &workspace);
            // Then only independently established fields and their actual owners are diagnosed.
            assert!(
                stderr.contains(&format!("invalid config ({count} problem")),
                "{stderr}"
            );
            assert!(
                stderr.contains(&root.path().join(bad_path).display().to_string()),
                "{stderr}"
            );
            assert!(stderr.contains(bad_key), "{stderr}");
            if let Some(key) = sibling_key {
                assert!(stderr.contains(key), "{stderr}");
                assert!(
                    stderr.contains(&root.path().join(other_path).display().to_string()),
                    "{stderr}"
                );
            } else {
                assert!(
                    !stderr.contains(&root.path().join(other_path).display().to_string()),
                    "{stderr}"
                );
            }
            assert!(!stderr.contains("missing field"), "{stderr}");
        }
    }
}

#[test]
fn config_check_literal_escalation_names_never_blame_valid_files() {
    // Given the reviewer's literal-name collision and its layer reversal.
    for reverse in [false, true] {
        let bad = "[profiles.x]\nharness = 'generic'\nescalate_to = 'truly-missing'\n";
        let good = "[profiles.'x.escalate_to']\nharness = 'generic'\n";
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
        let workspace = format!(
            "{workspace}[harnesses.generic]\ncommand = 'cat'\n[agents.omo]\ncommand = 'cat'\n"
        );
        // When the CLI attributes the structured escalation field.
        let (root, stderr) = check(user, &workspace);
        // Then only the actual offending file appears.
        assert!(stderr.contains("invalid config (1 problem)"), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "{}: profiles.x.escalate_to:",
                root.path().join(bad_path).display()
            )),
            "{stderr}"
        );
        assert!(
            !stderr.contains(&root.path().join(good_path).display().to_string()),
            "{stderr}"
        );
    }
}

#[test]
fn config_check_reports_self_contained_effort_errors_beside_syntax_failures() {
    // Given the independent review's fully supplied effort capabilities and either syntax owner.
    let readable = "[harnesses.omo]\ncommand = 'omo {model_args}'\nflags = 'omo'\n\
        effort_flag = '--thinking'\nefforts = ['high']\n\
        [profiles.omo]\nharness = 'omo'\neffort = 'not-an-effort'\n";
    for reverse in [false, true] {
        let (user, workspace, syntax_path, effort_path) = if reverse {
            (
                readable,
                "[ui\n",
                ".corral/corral.toml",
                "config/corral/config.toml",
            )
        } else {
            (
                "[ui\n",
                readable,
                "config/corral/config.toml",
                ".corral/corral.toml",
            )
        };
        // When the real CLI checks both inputs.
        let (root, stderr) = check(user, workspace);
        // Then both actual filenames and keys are visible.
        assert!(stderr.contains("invalid config (2 problems)"), "{stderr}");
        assert!(
            stderr.contains(&root.path().join(syntax_path).display().to_string()),
            "{stderr}"
        );
        assert!(stderr.contains("[ui"), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "{}: profiles.omo.effort:",
                root.path().join(effort_path).display()
            )),
            "{stderr}"
        );
    }
}

#[test]
fn config_check_ignores_shadowed_legacy_dependencies_not_their_schema_errors() {
    // Given the independent review's complete harness and unusable, unused legacy entry.
    let user = "[profiles.omo]\neffort = 'not-an-effort'\n";
    let workspace = "[harnesses.omo]\ncommand = 'omo {model_args}'\nflags = 'omo'\n\
        effort_flag = '--thinking'\nefforts = ['high']\n\
        [profiles.omo]\nharness = 'omo'\n[agents.omo]\ncommand = false\n";
    // When the real CLI resolves profile dependencies.
    let (root, stderr) = check(user, workspace);
    // Then both the supplied effort and the legacy schema error retain their owners.
    assert!(stderr.contains("invalid config (2 problems)"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "{}: profiles.omo.effort:",
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
    assert!(stderr.contains("agents.omo.command"), "{stderr}");
}

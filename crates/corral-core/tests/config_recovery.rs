use corral_core::{ConfigErrors, ConfigLayers};

fn problems(user: &str, workspace: &str) -> Vec<String> {
    let root = tempfile::tempdir().unwrap();
    let user_path = root.path().join("user.toml");
    let workspace_path = root.path().join("workspace.toml");
    std::fs::write(&user_path, user).unwrap();
    std::fs::write(&workspace_path, workspace).unwrap();
    let error = ConfigLayers::load_paths(&workspace_path, Some(&user_path))
        .unwrap()
        .validate(root.path())
        .unwrap_err();
    error.downcast_ref::<ConfigErrors>().unwrap().0.clone()
}

#[test]
fn required_field_recovery_retains_other_layer_flags_in_both_directions() {
    // Given invalid required commands and independently invalid flags, including literal names.
    for name in ["omo", "literal.harness", "literal flags", "quoted\"harness"] {
        let quoted = toml_edit::Key::new(name).to_string();
        for reverse in [false, true] {
            let flags = format!("[harnesses.{quoted}]\nflags = 'bogus'\n");
            let command = format!("[harnesses.{quoted}]\ncommand = false\n");
            let (user, workspace, flags_owner, command_owner) = if reverse {
                (&command, &flags, "workspace.toml", "user.toml")
            } else {
                (&flags, &command, "user.toml", "workspace.toml")
            };
            // When schema recovery cannot decode the harness.
            let errors = problems(user, &format!("{workspace}[agents.omo]\ncommand = 'cat'\n"));
            // Then both supplied fields keep their ownership, without a synthetic missing command.
            assert_eq!(errors.len(), 2, "{errors:#?}");
            for (owner, key) in [
                (flags_owner, format!("harnesses.{name}.flags")),
                (command_owner, format!("harnesses.{name}.command")),
            ] {
                assert!(
                    errors
                        .iter()
                        .any(|error| error.contains(owner) && error.contains(&key)),
                    "{errors:#?}"
                );
            }
            assert!(!errors.iter().any(|error| error.contains("missing field")));
        }
    }
}

#[test]
fn syntax_recovery_does_not_invent_unknown_profiles_in_either_layer() {
    // Given an unreadable inventory and a legal reference the other layer may complete.
    for reverse in [false, true] {
        let bad = "[profiles.omo\n";
        let good = "[defaults]\nprofile = 'omo'\n";
        let (user, workspace, owner) = if reverse {
            (bad, good, "user.toml")
        } else {
            (good, bad, "workspace.toml")
        };
        // When semantic checks run on the readable layer.
        let errors = problems(user, workspace);
        // Then the parse failure is the only established error.
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].contains(owner), "{errors:#?}");
        assert!(errors[0].contains("[profiles.omo"), "{errors:#?}");
    }
}

#[test]
fn incomplete_inventory_suppresses_dependent_references_not_independent_semantics() {
    // Given all profile-reference validators and independent invalid values in a readable workspace.
    let workspace = "[harnesses.generic]\ncommand = 'cat'\n\
        [profiles.next]\nharness = 'generic'\nescalate_to = 'unknown'\n\
        [lessons]\nprofile = 'unknown'\ntop = 0\n\
        [pm]\nagent = 'unknown'\n\
        [projects.app]\npath = '.'\ndefault_profile = 'unknown'\n\
        [projects.app.review]\nreviewer = 'unknown'\n\
        [review]\nreviewer = 'unknown'\ngate = ['']\n\
        [routing.reviewer_by_vendor]\nopenai = 'unknown'\nanthropic = 'next'\n'' = 'unknown'\n\
        [defaults]\nprofile = 'unknown'\n[limits]\nrunning = 0\n";
    // When the user inventory is unreadable.
    let errors = problems("[profiles.unknown\n", workspace);
    // Then unrelated semantics, including a blank vendor, still report their actual owners.
    assert_eq!(errors.len(), 5, "{errors:#?}");
    for key in [
        "lessons.top",
        "[review]",
        "routing.reviewer_by_vendor.",
        "limits.running",
    ] {
        assert!(
            errors
                .iter()
                .any(|error| error.contains("workspace.toml") && error.contains(key)),
            "{errors:#?}"
        );
    }
    assert!(!errors.iter().any(|error| error.contains("unknown profile")));
    assert!(
        errors
            .iter()
            .all(|error| !error.contains("unknown reviewer"))
    );
}

#[test]
fn complete_inventory_still_reports_truly_unknown_profile_references() {
    // Given a complete inventory with unknown references in each supported location.
    let workspace = "[harnesses.generic]\ncommand = 'cat'\n\
        [profiles.omo]\nharness = 'generic'\nescalate_to = 'missing'\n\
        [lessons]\nprofile = 'missing'\n[pm]\nagent = 'missing'\n\
        [projects.app]\npath = '.'\ndefault_profile = 'missing'\n\
        [projects.app.review]\nreviewer = 'missing'\n\
        [review]\nreviewer = 'missing'\n\
        [routing.reviewer_by_vendor]\nopenai = 'missing'\n\
        [defaults]\nprofile = 'missing'\n";
    // When both inventories are available.
    let errors = problems("", workspace);
    // Then all eight invalid references remain visible.
    assert_eq!(errors.len(), 8, "{errors:#?}");
    for key in [
        "profiles.omo.escalate_to",
        "lessons.profile",
        "pm.agent",
        "projects.app.default_profile",
        "[projects.app.review]",
        "[review]",
        "routing.reviewer_by_vendor.openai",
        "defaults.profile",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(key)),
            "{errors:#?}"
        );
    }
}

#[test]
fn required_harness_recovery_keeps_independent_escalation_errors() {
    // Given a required profile field and an unrelated escalation target in different layers.
    for reverse in [false, true] {
        let bad = "[profiles.omo]\nharness = false\n";
        let sibling = "[profiles.omo]\nescalate_to = 'truly-missing'\n";
        let (user, workspace, bad_owner, sibling_owner) = if reverse {
            (bad, sibling, "user.toml", "workspace.toml")
        } else {
            (sibling, bad, "workspace.toml", "user.toml")
        };
        // When the profile cannot be decoded but its escalation is independently checkable.
        let errors = problems(user, &format!("{workspace}[agents.omo]\ncommand = 'cat'\n"));
        // Then both actual fields are reported without synthetic missing-field errors.
        assert_eq!(errors.len(), 2, "{errors:#?}");
        for (owner, key) in [
            (bad_owner, "profiles.omo.harness"),
            (sibling_owner, "profiles.omo.escalate_to"),
        ] {
            assert!(
                errors
                    .iter()
                    .any(|error| error.contains(owner) && error.contains(key)),
                "{errors:#?}"
            );
        }
        assert!(!errors.iter().any(|error| error.contains("missing field")));
    }
}

#[test]
fn missing_capability_inputs_do_not_fabricate_model_errors() {
    // Given a partial harness whose model capability is unreadable or schema-invalid.
    for bad in ["[harnesses.custom\n", "[harnesses.custom]\nflags = false\n"] {
        for reverse in [false, true] {
            let good = "[harnesses.custom]\ncommand = 'runner {model_args}'\n\
                [profiles.omo]\nharness = 'custom'\nmodel = 'custom-model'\n";
            let (user, workspace, owner) = if reverse {
                (good, bad, "workspace.toml")
            } else {
                (bad, good, "user.toml")
            };
            // When a removed field would otherwise default to no model flag.
            let errors = problems(user, workspace);
            // Then only the actual malformed input is blamed.
            assert_eq!(errors.len(), 1, "{errors:#?}");
            assert!(errors[0].contains(owner), "{errors:#?}");
            assert!(!errors[0].contains("profiles.omo.model"), "{errors:#?}");
        }
    }
}

#[test]
fn rejected_harnesses_do_not_blame_valid_vendor_reviewer_mappings() {
    // Given a declared but undecodable harness required by a valid mapped profile.
    for reverse in [false, true] {
        let bad = "[harnesses.custom]\ncommand = false\n";
        let profile = "[profiles.omo]\nharness = 'custom'\nvendor = 'openai'\n";
        let (user, workspace, owner) = if reverse {
            (profile, bad, "workspace.toml")
        } else {
            (bad, profile, "user.toml")
        };
        let workspace = format!("{workspace}[routing.reviewer_by_vendor]\nopenai = 'omo'\n");
        // When reviewer resolution would otherwise use a lost harness declaration.
        let errors = problems(user, &workspace);
        // Then only the required command's schema error is independently established.
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].contains(owner), "{errors:#?}");
        assert!(
            errors[0].contains("harnesses.custom.command"),
            "{errors:#?}"
        );
    }
}

#[test]
fn rejected_harnesses_do_not_fabricate_legacy_command_conflicts() {
    // Given a schema-invalid preferred harness and a valid legacy fallback with different capabilities.
    for reverse in [false, true] {
        let bad = "[harnesses.custom]\ncommand = false\n";
        let profile = "[profiles.omo]\nharness = 'custom'\nargs = ['extra']\n";
        let (user, workspace, owner) = if reverse {
            (profile, bad, "workspace.toml")
        } else {
            (bad, profile, "user.toml")
        };
        let workspace = format!("{workspace}[agents.custom]\ncommand = 'cat'\n");
        // When recovery would temporarily select the lower-priority legacy command.
        let errors = problems(user, &workspace);
        // Then the fallback does not create profile or command accusations.
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].contains(owner), "{errors:#?}");
        assert!(
            errors[0].contains("harnesses.custom.command"),
            "{errors:#?}"
        );
    }
}

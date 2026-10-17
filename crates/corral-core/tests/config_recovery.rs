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
        !errors
            .iter()
            .any(|error| error.contains("unknown reviewer"))
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

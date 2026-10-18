use corral_core::{ConfigDoc, ConfigLayer, ConfigLayers};
use tempfile::{TempDir, tempdir};

struct Layers {
    root: TempDir,
    workspace: std::path::PathBuf,
    user: std::path::PathBuf,
}

impl Layers {
    fn new(user: &str, workspace: &str) -> Self {
        let root = tempdir().unwrap();
        let workspace_path = root.path().join("workspace.toml");
        let user_path = root.path().join("user.toml");
        std::fs::write(&workspace_path, workspace).unwrap();
        std::fs::write(&user_path, user).unwrap();
        Self {
            root,
            workspace: workspace_path,
            user: user_path,
        }
    }

    fn load(&self) -> ConfigLayers {
        ConfigLayers::load_paths(&self.workspace, Some(&self.user)).unwrap()
    }
}

const PERSONAL: &str = r#"
[ui]
mouse = false
done_window = "2w"
[harnesses.runner]
command = "runner {model_args}"
flags = "omo"
[profiles.omo]
harness = "runner"
model = "personal-model"
args = ["personal-one", "personal-two"]
[defaults]
profile = "omo"
"#;

#[test]
fn tables_merge_before_defaults_and_workspace_scalars_and_arrays_replace() {
    // Given disjoint leaves, a shared profile, and differing arrays in the two layers.
    let fixture = Layers::new(
        PERSONAL,
        r#"
[ui]
done_max = 17
[profiles.omo]
model = "workspace-model"
args = ["workspace-only"]
"#,
    );
    // When the raw tables are layered.
    let layers = fixture.load();
    let config = layers.config().unwrap();
    // Then inherited fields survive while explicit workspace values replace the user values.
    assert!(!config.ui.mouse);
    assert_eq!(config.ui.done_window, "2w");
    assert_eq!(config.ui.done_max, 17);
    assert_eq!(config.profiles["omo"].harness, "runner");
    assert_eq!(
        config.profiles["omo"].model.as_deref(),
        Some("workspace-model")
    );
    assert_eq!(config.profiles["omo"].args, ["workspace-only"]);
    assert_eq!(config.defaults.profile.as_deref(), Some("omo"));
    layers.validate(fixture.root.path()).unwrap();
}

#[test]
fn provenance_distinguishes_user_workspace_default_and_mixed_tables() {
    let fixture = Layers::new(PERSONAL, "[ui]\ndone_max = 17\n");
    let layers = fixture.load();
    assert_eq!(layers.source("ui.mouse").unwrap(), ConfigLayer::User);
    assert_eq!(
        layers.source("ui.done_max").unwrap(),
        ConfigLayer::Workspace
    );
    assert_eq!(
        layers.source("limits.running").unwrap(),
        ConfigLayer::Default
    );
    assert_eq!(layers.source("ui").unwrap(), ConfigLayer::Mixed);
}

#[test]
fn quoted_profile_keys_keep_their_provenance() {
    let fixture = Layers::new(
        "[profiles.\"my.profile\"]\nharness = 'generic'\n",
        "[harnesses.generic]\ncommand = 'cat'\n",
    );
    assert_eq!(
        fixture
            .load()
            .source("profiles.\"my.profile\".harness")
            .unwrap(),
        ConfigLayer::User
    );
}

#[test]
fn explicit_default_values_keep_their_layer_when_serde_omits_the_table() {
    let fixture = Layers::new("", "[board.card]\ncolumn_cost = false\n");
    assert_eq!(
        fixture.load().source("board.card.column_cost").unwrap(),
        ConfigLayer::Workspace
    );
}

#[test]
fn missing_user_file_is_optional_but_workspace_remains_required() {
    let fixture = Layers::new("", "[ui]\nmouse = false\n");
    std::fs::remove_file(&fixture.user).unwrap();
    assert!(!fixture.load().config().unwrap().ui.mouse);
    std::fs::remove_file(&fixture.workspace).unwrap();
    let error = ConfigLayers::load_paths(&fixture.workspace, Some(&fixture.user)).unwrap_err();
    assert!(format!("{error:#}").contains(&fixture.workspace.display().to_string()));
}

#[test]
fn forbidden_user_sections_are_rejected_even_when_workspace_replaces_them() {
    for section in [
        "projects.app",
        "project",
        "review",
        "limits",
        "agents",
        "pricing",
        "automation",
    ] {
        let fixture = Layers::new(&format!("[{section}]\n"), "");
        let error = fixture.load().config().unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains(&fixture.user.display().to_string()), "{text}");
        assert!(text.contains(section.split('.').next().unwrap()), "{text}");
    }
}

#[test]
fn syntax_errors_report_both_input_files() {
    let fixture = Layers::new("[ui\n", "[limits\n");
    let text = format!("{:#}", fixture.load().config().unwrap_err());
    assert!(text.contains(&fixture.user.display().to_string()), "{text}");
    assert!(
        text.contains(&fixture.workspace.display().to_string()),
        "{text}"
    );
}

#[test]
fn simultaneous_schema_errors_report_each_input_without_short_circuiting() {
    // Given invalid values in separate sections, the same section, or a partial profile.
    for (user, workspace, user_key, workspace_key) in [
        (
            "[ui]\nmouse = 'wrong-user-type'\n",
            "[limits]\nrunning = 'wrong-workspace-type'\n",
            "ui.mouse",
            "limits.running",
        ),
        (
            "[ui]\nmouse = 'wrong-user-type'\n",
            "[ui]\ndone_max = 'wrong-workspace-type'\n",
            "ui.mouse",
            "ui.done_max",
        ),
        (
            "[ui]\nmouse = 'wrong-user-type'\n",
            "[limits]\nrunning = 'wrong-workspace-type'\n[ui]\nmouse = true\n",
            "ui.mouse",
            "limits.running",
        ),
        (
            "[profiles.shared]\nmodel = false\n",
            "[profiles.shared]\nharness = 'generic'\nargs = false\n",
            "profiles.shared.model",
            "profiles.shared.args",
        ),
    ] {
        let fixture = Layers::new(user, workspace);
        // When both inputs are schema-checked together.
        let text = format!("{:#}", fixture.load().config().unwrap_err());
        // Then both actual owners and offending keys are present exactly once.
        for (path, key) in [
            (&fixture.user, user_key),
            (&fixture.workspace, workspace_key),
        ] {
            assert_eq!(
                text.matches(&path.display().to_string()).count(),
                1,
                "{text}"
            );
            assert!(text.contains(&format!("in `{key}`")), "{text}");
        }
        assert!(!text.contains("missing field"), "{text}");
    }
}

#[test]
fn unrelated_schema_failures_do_not_hide_missing_required_input_fields() {
    // Given an incomplete personal profile and an unrelated workspace type error.
    let fixture = Layers::new(
        "[profiles.personal]\nmodel = 'custom'\n",
        "[limits]\nrunning = 'wrong-workspace-type'\n",
    );
    // When schema checks aggregate both inputs.
    let text = format!("{:#}", fixture.load().config().unwrap_err());
    // Then a genuinely absent required field is not discarded as a partial fragment.
    assert!(text.contains(&fixture.user.display().to_string()), "{text}");
    assert!(text.contains("in `profiles.personal`"), "{text}");
    assert!(text.contains("missing field `harness`"), "{text}");
    assert!(
        text.contains(&fixture.workspace.display().to_string()),
        "{text}"
    );
    assert!(text.contains("in `limits.running`"), "{text}");
}

#[test]
fn canonical_project_sources_resolve_the_legacy_alias_without_rewriting() {
    // Given literal project names and a supported legacy spelling with comments.
    for name in ["app", "my.project", "my project", "quoted\"project"] {
        let quoted = toml_edit::Key::new(name).to_string();
        let original = format!(
            "# project notes\n[projects.{quoted}]\npath = '.'\ndefault_agent = 'custom' # keep alias\npost_land = []\n"
        );
        let fixture = Layers::new("", &original);
        let layers = fixture.load();
        // When canonical effective paths are attributed.
        let source = layers
            .source(&format!("projects.{quoted}.default_profile"))
            .unwrap();
        // Then the alias supplies the workspace value, including table provenance.
        assert_eq!(source, ConfigLayer::Workspace);
        assert_eq!(
            layers.source(&format!("projects.{quoted}")).unwrap(),
            ConfigLayer::Workspace
        );
        assert_eq!(
            layers.config().unwrap().projects[name]
                .default_profile
                .as_deref(),
            Some("custom")
        );
        assert_eq!(
            std::fs::read_to_string(&fixture.workspace).unwrap(),
            original
        );
    }
}

#[test]
fn routing_aliases_keep_atomic_array_provenance_and_duplicate_errors() {
    // Given each supported routing alias inside the array's two TOML spellings.
    for rules in [
        "[[routing.rules]]\ntitle = '^fix'\nbody = 'urgent'\nprofile = 'custom'\n",
        "[routing]\nrules = [{title = '^fix', body = 'urgent', profile = 'custom'}]\n",
    ] {
        let fixture = Layers::new("", rules);
        // When canonical effective routing values and their shared source are read.
        let layers = fixture.load();
        let config = layers.config().unwrap();
        // Then serde aliases retain their values and the whole array's workspace source.
        assert_eq!(config.routing.rules[0].title_regex.as_deref(), Some("^fix"));
        assert_eq!(
            config.routing.rules[0].body_regex.as_deref(),
            Some("urgent")
        );
        assert_eq!(
            layers.source("routing.rules").unwrap(),
            ConfigLayer::Workspace
        );
    }
    for (alias, canonical) in [("title", "title_regex"), ("body", "body_regex")] {
        let fixture = Layers::new(
            "",
            &format!(
                "[[routing.rules]]\n{alias} = 'one'\n{canonical} = 'two'\nprofile = 'custom'\n"
            ),
        );
        assert!(fixture.load().source("routing.rules").is_err());
    }
    let fixture = Layers::new(
        "",
        "[projects.app]\npath = '.'\ndefault_agent = 'one'\ndefault_profile = 'two'\n",
    );
    assert!(
        fixture
            .load()
            .source("projects.app.default_profile")
            .is_err()
    );
}

#[test]
fn masked_user_schema_errors_still_fail_validation() {
    let fixture = Layers::new("[ui]\nmouse = 'wrong'\n", "[ui]\nmouse = true\n");
    assert_schema_owner(&fixture, ConfigLayer::User, "ui.mouse");
    let text = format!(
        "{:#}",
        fixture.load().validate(fixture.root.path()).unwrap_err()
    );
    assert!(text.contains(&fixture.user.display().to_string()), "{text}");
    assert!(
        !text.contains(&fixture.workspace.display().to_string()),
        "{text}"
    );
}

fn assert_schema_owner(fixture: &Layers, owner: ConfigLayer, key: &str) {
    let text = format!("{:#}", fixture.load().config().unwrap_err());
    let (bad, valid) = match owner {
        ConfigLayer::User => (&fixture.user, &fixture.workspace),
        ConfigLayer::Workspace => (&fixture.workspace, &fixture.user),
        _ => panic!("schema errors must belong to an input file"),
    };
    assert_eq!(
        text.matches(&bad.display().to_string()).count(),
        1,
        "{text}"
    );
    assert!(!text.contains(&valid.display().to_string()), "{text}");
    assert!(text.contains(key), "{text}");
}

#[test]
fn workspace_schema_error_does_not_blame_unrelated_valid_user_keys() {
    let fixture = Layers::new(PERSONAL, "[limits]\nrunning = 'wrong-type'\n");
    assert_schema_owner(&fixture, ConfigLayer::Workspace, "limits.running");
}

#[test]
fn unmasked_user_schema_error_does_not_blame_valid_workspace_keys() {
    for name in ["bad.profile", "bad profile"] {
        let fixture = Layers::new(
            &format!("[profiles.\"{name}\"]\nharness = 'runner'\nargs = false\n"),
            "[ui]\nmouse = true\n[profiles.valid]\nharness = 'runner'\n",
        );
        assert_schema_owner(
            &fixture,
            ConfigLayer::User,
            &format!("profiles.{name}.args"),
        );
    }
}

#[test]
fn masked_user_semantics_and_workspace_errors_name_their_files() {
    let fixture = Layers::new(
        "[ui]\ndone_window = 'invalid'\n",
        "[ui]\ndone_window = '3d'\n[limits]\nrunning = 0\n",
    );
    let text = format!(
        "{:#}",
        fixture.load().validate(fixture.root.path()).unwrap_err()
    );
    assert!(
        text.contains(&format!("{}: ui.done_window", fixture.user.display())),
        "{text}"
    );
    assert!(
        text.contains(&format!("{}: limits.running", fixture.workspace.display())),
        "{text}"
    );
}

#[test]
fn semantic_errors_keep_user_ownership_for_dotted_and_spaced_profile_names() {
    for name in ["bad.profile", "bad profile"] {
        let fixture = Layers::new(
            &format!("[profiles.\"{name}\"]\nharness = 'missing'\n"),
            "[agents.omo]\ncommand = 'cat'\n",
        );
        let text = format!(
            "{:#}",
            fixture.load().validate(fixture.root.path()).unwrap_err()
        );
        assert!(
            text.contains(&format!(
                "{}: profiles.{name}.harness:",
                fixture.user.display()
            )),
            "{text}"
        );
        assert!(
            !text.contains(&format!(
                "{}: profiles.{name}.harness:",
                fixture.workspace.display()
            )),
            "{text}"
        );
    }
}

#[test]
fn mixed_validation_phases_report_both_layers_in_either_direction() {
    // Given syntax/schema, syntax/semantic, or schema/semantic failures in either layer.
    for (first, second, first_key, second_key) in [
        ("[ui\n", "[ui]\nmouse = 'wrong'\n", "ui", "ui.mouse"),
        (
            "[ui\n",
            "[ui]\ndone_window = 'wrong'\n",
            "ui",
            "ui.done_window",
        ),
        (
            "[ui]\nmouse = 'wrong'\n",
            "[ui]\ndone_window = 'wrong'\n",
            "ui.mouse",
            "ui.done_window",
        ),
    ] {
        for reverse in [false, true] {
            let (user, workspace, user_key, workspace_key) = if reverse {
                (second, first, second_key, first_key)
            } else {
                (first, second, first_key, second_key)
            };
            let fixture = Layers::new(user, workspace);
            // When validation checks both independently readable files.
            let text = format!(
                "{:#}",
                fixture.load().validate(fixture.root.path()).unwrap_err()
            );
            // Then each file and its offending key are reported without invented dependencies.
            for (path, key) in [
                (&fixture.user, user_key),
                (&fixture.workspace, workspace_key),
            ] {
                assert!(text.contains(&path.display().to_string()), "{text}");
                assert!(text.contains(key), "{text}");
            }
            assert!(!text.contains("unknown profile"), "{text}");
        }
    }
}

#[test]
fn shared_profile_semantics_name_only_the_field_owner_even_when_masked() {
    // Given a profile split across layers, including literal names resembling nested paths.
    for name in [
        "omo",
        "my.profile",
        "my profile",
        "quoted\"profile",
        "omo.effort",
    ] {
        let quoted = toml_edit::Key::new(name).to_string();
        for masked in [false, true] {
            for reverse in [false, true] {
                let bad = format!("[profiles.{quoted}]\neffort = 'not-an-effort'\n");
                let good = format!(
                    "[harnesses.omo]\ncommand = 'omo {{model_args}}'\nflags = 'omo'\n\
                     [profiles.{quoted}]\nharness = 'omo'\n{}{}",
                    if masked { "effort = 'high'\n" } else { "" },
                    if name == "omo" {
                        ""
                    } else {
                        "[profiles.omo]\nharness = 'omo'\n"
                    }
                );
                let (user, workspace) = if reverse {
                    (&good, &bad)
                } else {
                    (&bad, &good)
                };
                let fixture = Layers::new(user, workspace);
                // When semantics check effective and masked personal values.
                let error = fixture.load().validate(fixture.root.path()).unwrap_err();
                let problems = &error.downcast_ref::<corral_core::ConfigErrors>().unwrap().0;
                // Then only the invalid effort field's file is blamed.
                assert_eq!(problems.len(), 1, "{problems:#?}");
                let owner = if reverse {
                    &fixture.workspace
                } else {
                    &fixture.user
                };
                assert!(
                    problems[0]
                        .starts_with(&format!("{}: profiles.{name}.effort:", owner.display())),
                    "{problems:#?}"
                );
            }
        }
    }
}

#[test]
fn shared_profile_model_and_args_errors_keep_contributing_field_paths() {
    // Given user model/args and a workspace harness that cannot pass them.
    let fixture = Layers::new(
        "[profiles.omo]\nmodel = 'custom'\nargs = ['extra']\n",
        "[harnesses.generic]\ncommand = 'cat'\n[profiles.omo]\nharness = 'generic'\n",
    );
    // When profile semantics inspect both contributions.
    let error = fixture.load().validate(fixture.root.path()).unwrap_err();
    let problems = &error.downcast_ref::<corral_core::ConfigErrors>().unwrap().0;
    // Then the invalid model and incompatible args/command each name their actual field.
    assert_eq!(problems.len(), 3, "{problems:#?}");
    for (path, key) in [
        (&fixture.user, "profiles.omo.model"),
        (&fixture.user, "profiles.omo.args"),
        (&fixture.workspace, "harnesses.generic.command"),
    ] {
        assert!(
            problems
                .iter()
                .any(|problem| problem.starts_with(&format!("{}: {key}:", path.display()))),
            "{problems:#?}"
        );
    }
}

#[test]
fn schema_recovery_keeps_legal_partial_profiles_and_independent_semantics() {
    // Given a split profile, an invalid optional field, and independent semantics in both layers.
    let fixture = Layers::new(
        "[profiles.omo]\nmodel = false\neffort = 'wrong'\n",
        "[harnesses.omo]\ncommand = 'omo {model_args}'\n\
         [profiles.omo]\nharness = 'omo'\n[limits]\nrunning = 0\n",
    );
    // When checking continues after schema decoding fails.
    let text = format!(
        "{:#}",
        fixture.load().validate(fixture.root.path()).unwrap_err()
    );
    // Then all independent supplied errors survive, without a synthetic missing harness.
    for (path, key) in [
        (&fixture.user, "profiles.omo.model"),
        (&fixture.user, "profiles.omo.effort"),
        (&fixture.workspace, "limits.running"),
    ] {
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert!(text.contains(key), "{text}");
    }
    assert!(!text.contains("missing field"), "{text}");
}

#[test]
fn incomplete_harness_inputs_do_not_manufacture_profile_reference_errors() {
    // Given a valid profile reference whose harness cannot yet be decoded.
    for workspace in [
        "[harnesses.runner]\ncommand = false\n",
        "[harnesses.runner\n",
    ] {
        let fixture = Layers::new("[profiles.omo]\nharness = 'runner'\n", workspace);
        // When independent schema or syntax errors are retained.
        let error = fixture.load().validate(fixture.root.path()).unwrap_err();
        let problems = &error.downcast_ref::<corral_core::ConfigErrors>().unwrap().0;
        // Then the unreadable harness is the only problem, not the valid referring file.
        assert_eq!(problems.len(), 1, "{problems:#?}");
        assert!(
            problems[0].starts_with(&fixture.workspace.display().to_string()),
            "{problems:#?}"
        );
    }
}

#[test]
fn forbidden_user_sections_do_not_hide_provable_workspace_profile_errors() {
    // Given two readable inventories and an unrelated user-layer policy violation.
    for (workspace, key) in [
        (
            "[profiles.omo]\nharness = 'missing'\n",
            "profiles.omo.harness",
        ),
        ("[profiles.omo]\nmodel = 'custom'\n", "profiles.omo"),
    ] {
        let fixture = Layers::new("[limits]\nrunning = 3\n", workspace);
        // When validation distinguishes policy errors from missing input.
        let error = fixture.load().validate(fixture.root.path()).unwrap_err();
        let problems = &error.downcast_ref::<corral_core::ConfigErrors>().unwrap().0;
        // Then both actual problems identify their owning file and key.
        assert_eq!(problems.len(), 2, "{problems:#?}");
        for (path, key) in [(&fixture.user, "limits"), (&fixture.workspace, key)] {
            assert!(
                problems.iter().any(|problem| {
                    problem.starts_with(&path.display().to_string()) && problem.contains(key)
                }),
                "{problems:#?}"
            );
        }
        assert!(
            problems.iter().any(|problem| problem.contains("harness")),
            "{problems:#?}"
        );
    }
}

#[test]
fn declared_but_undecodable_profiles_do_not_blame_valid_defaults_in_either_layer() {
    // Given a valid default and a declared profile whose required field is invalid.
    for reverse in [false, true] {
        let bad = "[profiles.omo]\nharness = false\n";
        let good = "[defaults]\nprofile = 'omo'\n";
        let (user, workspace) = if reverse { (bad, good) } else { (good, bad) };
        let fixture = Layers::new(user, workspace);
        // When recovery removes the undecodable profile from its temporary candidate.
        let error = fixture.load().validate(fixture.root.path()).unwrap_err();
        let problems = &error.downcast_ref::<corral_core::ConfigErrors>().unwrap().0;
        // Then only the schema-invalid field is diagnosed, never the valid reference.
        let owner = if reverse {
            &fixture.user
        } else {
            &fixture.workspace
        };
        assert_eq!(problems.len(), 1, "{problems:#?}");
        assert!(
            problems[0].starts_with(&owner.display().to_string()),
            "{problems:#?}"
        );
        assert!(
            problems[0].contains("profiles.omo.harness"),
            "{problems:#?}"
        );
    }
}

#[test]
fn rejected_declarations_preserve_all_profile_references_without_hiding_real_errors() {
    // Given every profile-reference validator points at a declared but undecodable profile.
    let fixture = Layers::new(
        "[profiles.'broken.profile']\nharness = false\n",
        "[harnesses.generic]\ncommand = 'cat'\n\
         [profiles.next]\nharness = 'generic'\nescalate_to = 'broken.profile'\n\
         [lessons]\nprofile = 'broken.profile'\n\
         [pm]\nagent = 'broken.profile'\n\
         [projects.app]\npath = '.'\ndefault_profile = 'broken.profile'\n\
         [projects.app.review]\nreviewer = 'broken.profile'\n\
         [review]\nreviewer = 'broken.profile'\n\
         [routing.reviewer_by_vendor]\nopenai = 'broken.profile'\n\
         [defaults]\nprofile = 'truly-missing'\n[limits]\nrunning = 0\n",
    );
    // When schema recovery retains declaration identity for semantic checks.
    let error = fixture.load().validate(fixture.root.path()).unwrap_err();
    let problems = &error.downcast_ref::<corral_core::ConfigErrors>().unwrap().0;
    // Then the schema error, a truly unknown profile, and unrelated semantics survive.
    assert_eq!(problems.len(), 3, "{problems:#?}");
    for (path, key) in [
        (&fixture.user, "profiles.broken.profile.harness"),
        (&fixture.workspace, "defaults.profile"),
        (&fixture.workspace, "limits.running"),
    ] {
        assert!(
            problems.iter().any(|problem| {
                problem.starts_with(&path.display().to_string()) && problem.contains(key)
            }),
            "{problems:#?}"
        );
    }
}

#[test]
fn effective_delta_does_not_materialize_other_inherited_values() {
    let fixture = Layers::new(PERSONAL, "# workspace notes\n");
    let base = fixture.load().config().unwrap();
    let mut draft = base.clone();
    draft.ui.mouse = true;
    let mut doc = ConfigDoc::parse("# workspace notes\n").unwrap();
    doc.apply_config_changes(&base, &draft).unwrap();
    let expected = ConfigDoc::parse("[ui]\nmouse = true\n")
        .unwrap()
        .config()
        .unwrap();
    assert_eq!(doc.config().unwrap(), expected);
    assert!(doc.to_string().contains("# workspace notes\n"));
    assert!(doc.get("profiles").unwrap().is_none());
    assert!(doc.get("ui.done_window").unwrap().is_none());
}

#[test]
fn resetting_last_board_option_preserves_a_concurrent_sibling_edit() {
    let base = ConfigDoc::parse("[board.card]\ncolumn_cost = true\n")
        .unwrap()
        .config()
        .unwrap();
    let mut draft = base.clone();
    draft.board.card.column_cost = false;
    let mut latest =
        ConfigDoc::parse("[board.card]\ncolumn_cost = true\ncost_warn = 17.0\n").unwrap();
    latest.apply_config_changes(&base, &draft).unwrap();
    let saved = latest.config().unwrap();
    assert!(!saved.board.card.column_cost);
    assert_eq!(saved.board.card.cost_warn, Some(17.0));
}

#[test]
fn deleting_last_map_entry_preserves_a_concurrently_added_entry() {
    let base = ConfigDoc::parse("[profiles.old]\nharness = 'generic'\n")
        .unwrap()
        .config()
        .unwrap();
    let mut draft = base.clone();
    draft.profiles.clear();
    let mut latest =
        ConfigDoc::parse("[profiles.old]\nharness = 'generic'\n[profiles.new]\nharness = 'omo'\n")
            .unwrap();
    latest.apply_config_changes(&base, &draft).unwrap();
    let saved = latest.config().unwrap();
    assert!(!saved.profiles.contains_key("old"));
    assert_eq!(saved.profiles["new"].harness, "omo");
}

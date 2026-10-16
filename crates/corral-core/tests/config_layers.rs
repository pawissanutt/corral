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
fn masked_user_schema_errors_still_fail_validation() {
    let fixture = Layers::new("[ui]\nmouse = 'wrong'\n", "[ui]\nmouse = true\n");
    let text = format!(
        "{:#}",
        fixture.load().validate(fixture.root.path()).unwrap_err()
    );
    assert!(text.contains(&fixture.user.display().to_string()), "{text}");
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
            text.contains(&format!("{}: profiles.{name}:", fixture.user.display())),
            "{text}"
        );
        assert!(
            !text.contains(&format!(
                "{}: profiles.{name}:",
                fixture.workspace.display()
            )),
            "{text}"
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

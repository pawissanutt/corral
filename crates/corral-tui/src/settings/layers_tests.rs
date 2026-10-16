use std::process::Command;

use corral_core::{ConfigLayer, ConfigLayers, Store};

use super::SettingsView;

use super::model::Field;
use super::mouse_tests::{click, find, screen};
use super::tests::{self, c, fixture, press, render, render_at, type_value};
use crate::app::App;
use crate::{Action, Mode, Tone, View};

/// User writes run in a child with private HOME/XDG, never against the owner's
/// preferences and never by mutating the parallel test process's environment.
fn isolated(name: &str) -> bool {
    if std::env::var("CORRAL_SETTINGS_LAYER_TEST").as_deref() == Ok(name) {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("settings::layers_tests::{name}"),
            "--nocapture",
        ])
        .env("CORRAL_SETTINGS_LAYER_TEST", name)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
    true
}

fn personal_fixture() -> tests::Fixture {
    let mut fx = fixture();
    let path = ConfigLayers::user_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        "# personal preferences\n[ui]\nmouse = true # pointer preference\ndone_window = \"13d\"\n",
    )
    .unwrap();
    fx.ctx
        .store
        .edit_config(|doc| {
            doc.unset("ui.mouse")?;
            doc.unset("ui.done_window")?;
            doc.unset("ui.done_max")?;
            Ok(())
        })
        .unwrap();
    fx.ctx.store = Store::open(&fx.root).unwrap();
    fx
}

fn user_text() -> String {
    std::fs::read_to_string(ConfigLayers::user_path().unwrap()).unwrap()
}

fn legacy_project_fixture() -> tests::Fixture {
    let mut fx = personal_fixture();
    let legacy = fx.toml().replace(
        "default_profile = \"claude\"",
        "# project profile choice\ndefault_agent = \"claude\" # legacy selection",
    );
    assert!(legacy.contains("default_agent"));
    std::fs::write(fx.ctx.store.config_path(), legacy).unwrap();
    fx.ctx.store = Store::open(&fx.root).unwrap();
    fx
}

#[test]
fn changing_legacy_project_profile_saves_and_reopens() {
    if isolated("changing_legacy_project_profile_saves_and_reopens") {
        return;
    }
    // Given a legacy selection, personal preferences and a concurrent sibling edit.
    let mut fx = legacy_project_fixture();
    let personal = user_text();
    let mut view = SettingsView::new(&fx.ctx);
    view.on_key(c('4'), &fx.ctx);
    view.select_field(&Field::ProjectProfile("web".into()));
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("projects.web.base", "concurrent"))
        .unwrap();
    // When the profile is changed from claude to omo and saved through settings.
    press(
        &mut view,
        &fx.ctx,
        &[
            tests::k(ratatui::crossterm::event::KeyCode::Right),
            tests::k(ratatui::crossterm::event::KeyCode::Right),
        ],
    );
    assert_eq!(
        view.draft.projects["web"].default_profile.as_deref(),
        Some("omo")
    );
    assert!(matches!(view.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    // Then a fresh settings instance reads the selection and retained sibling/comments.
    fx.ctx.store = Store::open(&fx.root).unwrap();
    let reopened = SettingsView::new(&fx.ctx);
    assert_eq!(
        reopened.draft.projects["web"].default_profile.as_deref(),
        Some("omo")
    );
    assert_eq!(
        reopened.draft.projects["web"].base.as_deref(),
        Some("concurrent")
    );
    let text = fx.toml();
    assert!(!text.contains("default_agent"), "{text}");
    assert!(
        text.contains("default_profile = \"omo\" # legacy selection"),
        "{text}"
    );
    assert!(text.contains("# project profile choice"), "{text}");
    assert!(text.contains(tests::HEAD_COMMENT), "{text}");
    assert_eq!(user_text(), personal);
    assert!(!view.dirty());
    assert!(!reopened.dirty());
}

#[test]
fn clearing_legacy_project_profile_saves_and_reopens() {
    if isolated("clearing_legacy_project_profile_saves_and_reopens") {
        return;
    }
    // Given an explicit legacy project selection above the global omp default.
    let mut fx = legacy_project_fixture();
    let personal = user_text();
    let mut view = SettingsView::new(&fx.ctx);
    view.on_key(c('4'), &fx.ctx);
    view.select_field(&Field::ProjectProfile("web".into()));
    // When the selection is cleared to inherit and saved through settings.
    view.on_key(tests::k(ratatui::crossterm::event::KeyCode::Left), &fx.ctx);
    assert!(view.draft.projects["web"].default_profile.is_none());
    assert!(matches!(view.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    // Then a fresh settings instance has no project override and inherits omp.
    fx.ctx.store = Store::open(&fx.root).unwrap();
    let reopened = SettingsView::new(&fx.ctx);
    assert!(reopened.draft.projects["web"].default_profile.is_none());
    assert_eq!(reopened.draft.default_profile(None, Some("web")), "omp");
    assert_eq!(reopened.draft.projects["web"].base.as_deref(), Some("main"));
    let layers = fx.ctx.store.config_layers().unwrap();
    let doc = layers.doc(ConfigLayer::Workspace).unwrap();
    assert!(doc.get("projects.web.default_agent").unwrap().is_none());
    assert!(doc.get("projects.web.default_profile").unwrap().is_none());
    let text = fx.toml();
    assert!(text.contains("# project profile choice"), "{text}");
    assert!(text.contains("# legacy selection"), "{text}");
    assert!(text.contains(tests::HEAD_COMMENT), "{text}");
    assert_eq!(user_text(), personal);
    assert!(!view.dirty());
    assert!(!reopened.dirty());
}

#[test]
fn clearing_inherited_profile_args_saves_an_empty_workspace_override() {
    if isolated("clearing_inherited_profile_args_saves_an_empty_workspace_override") {
        return;
    }
    let mut fx = personal_fixture();
    let personal = format!("{}\n[profiles.omo]\nargs = [\"--personal\"]\n", user_text());
    std::fs::write(ConfigLayers::user_path().unwrap(), &personal).unwrap();
    fx.ctx.store = Store::open(&fx.root).unwrap();
    let mut view = SettingsView::new(&fx.ctx);
    view.set_section(1);
    view.select_field(&Field::Profile("omo".into()));
    press(
        &mut view,
        &fx.ctx,
        &[tests::k(ratatui::crossterm::event::KeyCode::Enter)],
    );
    view.select_field(&Field::ProfileArgs("omo".into()));
    assert_eq!(view.draft.profiles["omo"].args, ["--personal"]);
    type_value(&mut view, &fx.ctx, "");
    assert!(view.draft.profiles["omo"].args.is_empty());
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("ui.done_max", "47"))
        .unwrap();

    assert!(matches!(view.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));

    let layers = fx.ctx.store.config_layers().unwrap();
    assert!(layers.config().unwrap().profiles["omo"].args.is_empty());
    assert_eq!(layers.config().unwrap().ui.done_max, 47);
    assert_eq!(
        layers.source("profiles.omo.args").unwrap(),
        ConfigLayer::Workspace
    );
    assert_eq!(user_text(), personal);
    assert!(fx.toml().contains(tests::HEAD_COMMENT));
    assert!(
        layers
            .doc(ConfigLayer::Workspace)
            .unwrap()
            .get("profiles.omo.args")
            .unwrap()
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(view.draft.profiles["omo"].args.is_empty());
    assert!(!view.dirty());
    fx.ctx.store = Store::open(&fx.root).unwrap();
    let reopened = SettingsView::new(&fx.ctx);
    assert!(reopened.draft.profiles["omo"].args.is_empty());
}

#[test]
fn keyboard_workspace_save_keeps_inherited_values_out_of_workspace() {
    if isolated("keyboard_workspace_save_keeps_inherited_values_out_of_workspace") {
        return;
    }
    // Given a user preference and an unrelated concurrent workspace edit.
    let fx = personal_fixture();
    let personal = user_text();
    let mut view = SettingsView::new(&fx.ctx);
    assert_eq!(view.save_target, ConfigLayer::Workspace);
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("sync.idle_after_secs", "97"))
        .unwrap();
    // When the default target is saved through the keyboard.
    type_value(&mut view, &fx.ctx, "8");
    assert!(matches!(view.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    // Then only the workspace delta is written, preserving comments and concurrency.
    let layers = fx.ctx.store.config_layers().unwrap();
    let cfg = layers.config().unwrap();
    assert_eq!((cfg.limits.running, cfg.sync.idle_after_secs), (8, 97));
    assert_eq!(layers.source("ui.done_window").unwrap(), ConfigLayer::User);
    assert!(!fx.toml().contains("done_window"));
    assert!(fx.toml().contains(tests::HEAD_COMMENT));
    assert_eq!(user_text(), personal);
}

#[test]
fn keyboard_user_save_preserves_comments_and_concurrent_personal_keys() {
    if isolated("keyboard_user_save_preserves_comments_and_concurrent_personal_keys") {
        return;
    }
    // Given an open personal draft and another writer changing a sibling key.
    let fx = personal_fixture();
    let workspace = fx.toml();
    let mut view = SettingsView::new(&fx.ctx);
    let path = ConfigLayers::user_path().unwrap();
    std::fs::write(path, format!("{}done_max = 23\n", user_text())).unwrap();
    view.select_field(&Field::Mouse);
    // When the personal control is changed, user selected, and saved by key.
    press(&mut view, &fx.ctx, &[c(' '), c('u')]);
    let staged = render(&mut view, &fx.ctx);
    assert!(staged.contains("source: user · staged → user"), "{staged}");
    assert!(matches!(view.on_key(c('s'), &fx.ctx), Action::Toast(t) if t.tone == Tone::Good));
    // Then the intended file changes and the concurrent key and comments survive.
    let layers = fx.ctx.store.config_layers().unwrap();
    let cfg = layers.config().unwrap();
    assert!(!cfg.ui.mouse);
    assert_eq!(cfg.ui.done_max, 23);
    assert_eq!(layers.source("ui.mouse").unwrap(), ConfigLayer::User);
    assert_eq!(fx.toml(), workspace);
    assert!(user_text().contains("# personal preferences"));
    assert!(user_text().contains("# pointer preference"));
    assert!(!view.dirty());
}

#[test]
fn mouse_selects_user_target_and_saves_personal_edit() {
    if isolated("mouse_selects_user_target_and_saves_personal_edit") {
        return;
    }
    // Given settings on its default workspace target.
    let fx = personal_fixture();
    let workspace = fx.toml();
    let mut app = App::new(fx.ctx.clone(), Mode::Settings).unwrap();
    let before = screen(&mut app);
    // When the visible target, toggle, and save controls are clicked.
    click(&mut app, find(&before, "u user", "u user"));
    click(&mut app, find(&before, "Mouse", " off "));
    let staged = screen(&mut app);
    click(&mut app, find(&staged, "w workspace", "s save"));
    // Then user is written, not workspace, and the displayed source is user.
    assert!(!Store::open(&fx.root).unwrap().config.ui.mouse);
    assert_eq!(fx.toml(), workspace);
    assert!(user_text().contains("mouse = false"));
    let saved = screen(&mut app);
    assert!(!saved.contains("unsaved changes"), "{saved}");
}

#[test]
fn user_save_rejects_mixed_draft_without_losing_either_edit() {
    if isolated("user_save_rejects_mixed_draft_without_losing_either_edit") {
        return;
    }
    // Given both personal and workspace edits in one draft.
    let fx = personal_fixture();
    let workspace = fx.toml();
    let user = user_text();
    let mut view = SettingsView::new(&fx.ctx);
    type_value(&mut view, &fx.ctx, "9");
    view.select_field(&Field::Mouse);
    press(&mut view, &fx.ctx, &[c(' '), c('u')]);
    let draft = view.draft.clone();
    // When saving to user is attempted.
    let result = view.on_key(c('s'), &fx.ctx);
    // Then neither file is partially saved and the draft can still save to workspace.
    assert!(matches!(result, Action::Toast(t) if t.tone == Tone::Bad));
    assert_eq!(view.draft, draft);
    assert_eq!(fx.toml(), workspace);
    assert_eq!(user_text(), user);
    assert!(
        matches!(press(&mut view, &fx.ctx, &[c('w'), c('s')]), Action::Toast(t) if t.tone == Tone::Good)
    );
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!(cfg.limits.running, 9);
    assert!(!cfg.ui.mouse);
}

#[test]
fn user_save_rejects_workspace_deletion_absent_from_user_file() {
    if isolated("user_save_rejects_workspace_deletion_absent_from_user_file") {
        return;
    }
    // Given a workspace-only project removed from the draft.
    let fx = personal_fixture();
    let workspace = fx.toml();
    let user = user_text();
    let mut view = SettingsView::new(&fx.ctx);
    view.on_key(c('4'), &fx.ctx);
    view.select_field(&Field::Project("web".into()));
    press(&mut view, &fx.ctx, &[c('x'), c('u')]);
    // When user is asked to save a deletion it cannot represent.
    let result = view.on_key(c('s'), &fx.ctx);
    // Then it is rejected rather than silently dropping the removal.
    assert!(matches!(result, Action::Toast(t) if t.tone == Tone::Bad));
    assert!(!view.draft.projects.contains_key("web"));
    assert!(view.dirty());
    assert_eq!(fx.toml(), workspace);
    assert_eq!(user_text(), user);
}

#[test]
fn masked_user_save_reloads_effective_workspace_value_and_source() {
    if isolated("masked_user_save_reloads_effective_workspace_value_and_source") {
        return;
    }
    // Given an explicit workspace value masking the selected user target.
    let fx = personal_fixture();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("ui.mouse", "true"))
        .unwrap();
    let mut view = SettingsView::new(&fx.ctx);
    view.select_field(&Field::Mouse);
    // When a different user value is saved.
    let result = press(&mut view, &fx.ctx, &[c(' '), c('u'), c('s')]);
    // Then the raw preference is saved but effective value and source stay workspace.
    assert!(
        matches!(result, Action::Toast(t) if t.tone == Tone::Good && t.body.contains("user") && t.body.contains("Other layers"))
    );
    assert!(user_text().contains("mouse = false"));
    assert!(view.draft.ui.mouse);
    assert_eq!(
        fx.ctx
            .store
            .config_layers()
            .unwrap()
            .source("ui.mouse")
            .unwrap(),
        ConfigLayer::Workspace
    );
    assert!(!view.dirty());
}

#[test]
fn keyed_rows_render_default_user_workspace_and_mixed_provenance() {
    if isolated("keyed_rows_render_default_user_workspace_and_mixed_provenance") {
        return;
    }
    // Given a dotted profile assembled from user model and workspace harness.
    let mut fx = personal_fixture();
    fx.ctx
        .store
        .edit_config(|doc| {
            doc.set_str("profiles.\"fast.local\".harness", "claude")?;
            doc.set_str("harnesses.claude.flags", "claude")?;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        ConfigLayers::user_path().unwrap(),
        format!(
            "{}\n[profiles.\"fast.local\"]\nmodel = \"sonnet\"\n",
            user_text()
        ),
    )
    .unwrap();
    fx.ctx.store = Store::open(&fx.root).unwrap();
    let mut view = SettingsView::new(&fx.ctx);
    // When the real form lines are rendered across sections and a profile editor.
    let general = render(&mut view, &fx.ctx);
    assert!(general.contains("source: user"), "{general}");
    assert!(general.contains("source: workspace"), "{general}");
    view.on_key(c('7'), &fx.ctx);
    let cards = render(&mut view, &fx.ctx);
    assert!(cards.contains("source: default"), "{cards}");
    view.on_key(c('2'), &fx.ctx);
    view.open_profile("fast.local".into());
    let editor = render(&mut view, &fx.ctx);
    // Then actual field paths, not validation prefixes, determine provenance.
    for (field, source) in [
        (Field::Profile("fast.local".into()), "mixed"),
        (Field::ProfileHarness("fast.local".into()), "workspace"),
        (Field::ProfileModel("fast.local".into()), "user"),
        (Field::ProfileEffort("fast.local".into()), "default"),
    ] {
        let lines = view.form_lines(&fx.ctx, 140);
        let row = view.rows().iter().position(|r| r.field == field).unwrap();
        assert!(
            lines
                .iter()
                .filter(|l| l.row == Some(row))
                .any(|l| { l.line.to_string().contains(&format!("source: {source}")) }),
            "{field:?}: {editor}"
        );
    }
    for (w, h) in [(100, 28), (160, 45)] {
        println!(
            "Layered profile {w}x{h}:\n{}",
            render_at(&mut view, &fx.ctx, w, h)
        );
    }
    view.on_key(c('3'), &fx.ctx);
    let flags_row = view
        .rows()
        .iter()
        .position(|r| r.field == Field::HarnessFlags("claude".into()))
        .unwrap();
    assert!(view.form_lines(&fx.ctx, 140).iter().any(|line| {
        line.row == Some(flags_row) && line.line.to_string().contains("source: workspace")
    }));
    // Every keyed row, including Review's validation-only prefixes, must resolve
    // a real TOML path and carry provenance in its rendered row.
    for section in super::model::SECTIONS {
        for row in view.rows_of(section) {
            if !row.key.is_empty() {
                let key = super::layers::key(&row, &view.draft).unwrap();
                fx.ctx.store.config_layers().unwrap().source(&key).unwrap();
            }
        }
    }
}

#[test]
fn project_profile_row_attributes_a_legacy_alias_to_workspace() {
    if isolated("project_profile_row_attributes_a_legacy_alias_to_workspace") {
        return;
    }
    // Given an existing project's nondefault profile configured through its legacy alias.
    let mut fx = personal_fixture();
    fx.ctx
        .store
        .edit_config(|doc| {
            doc.unset("projects.web.default_profile")?;
            doc.set_str("projects.web.default_agent", "claude")
        })
        .unwrap();
    fx.ctx.store = Store::open(&fx.root).unwrap();
    let original = fx.toml();
    let mut view = SettingsView::new(&fx.ctx);
    view.on_key(c('4'), &fx.ctx);
    let row = view
        .rows()
        .iter()
        .position(|r| r.field == Field::ProjectProfile("web".into()))
        .unwrap();
    // When the actual settings form annotates the canonical project profile row.
    let lines = view.form_lines(&fx.ctx, 140);
    // Then its value and source identify the workspace alias without rewriting it.
    assert_eq!(
        view.draft.projects["web"].default_profile.as_deref(),
        Some("claude")
    );
    assert!(lines.iter().any(|line| {
        line.row == Some(row) && line.line.to_string().contains("source: workspace")
    }));
    assert_eq!(fx.toml(), original);
}

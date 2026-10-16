use corral_core::config_doc::toml_edit::{Table, value};
use corral_core::{ConfigDoc, ConfigErrors, Store};
use tempfile::{TempDir, tempdir};

const COMMENTED: &str = r#"# corral workspace config
# (hand-written; keep these notes)

[limits]
running = 3   # global slots
# per-project caps below
per_project = { app = 2 }

[projects.app]
# the main repo
path = "app"
default_profile = "omo"

[harnesses.omo]
command = "omo -n '{name}' {model_args} \"$(cat '{prompt_file}')\""

[harnesses.claude]
command = "claude {model_args} \"$(cat '{prompt_file}')\""

# Profiles: harness + model + effort.
[profiles.omo]
harness = "omo"

[profiles.deep]
harness = "claude"
effort = "high"     # claude levels: low..max

[artifacts]
globs = [
    ".omo/evidence/**",   # agent evidence
    "**/*report*.md",
]
max_bytes = 20000000
"#;

fn workspace(toml: &str) -> (TempDir, Store) {
    let d = tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("app")).unwrap();
    std::fs::create_dir_all(d.path().join(".corral")).unwrap();
    std::fs::write(d.path().join(".corral/corral.toml"), toml).unwrap();
    let s = Store::open(d.path()).unwrap();
    (d, s)
}

fn on_disk(s: &Store) -> String {
    std::fs::read_to_string(s.config_path()).unwrap()
}

fn problems(err: &anyhow::Error) -> Vec<String> {
    err.chain()
        .find_map(|e| e.downcast_ref::<ConfigErrors>())
        .map(|e| e.0.clone())
        .unwrap_or_else(|| panic!("not a validation error: {err:#}"))
}

const LEGACY_PROJECT: &str = "# workspace notes\n[projects.\"my.app\"]\npath = '.' # repository\n# profile choice\ndefault_agent = 'custom' # legacy note\nbase = 'concurrent' # branch notes\nowner_hint = 'raw-only'\n";

#[test]
fn semantic_profile_change_replaces_a_raw_alias_and_keeps_its_comments() {
    // Given a canonical snapshot and its original, comment-bearing legacy document.
    let mut doc = ConfigDoc::parse(LEGACY_PROJECT).unwrap();
    let base = doc.config().unwrap();
    let mut draft = base.clone();
    draft.projects.get_mut("my.app").unwrap().default_profile = Some("omo".into());
    // When only that canonical field changes.
    doc.apply_config_changes(&base, &draft).unwrap();
    // Then the raw alias is replaced in place, with every unrelated byte preserved.
    assert_eq!(
        doc.to_string(),
        LEGACY_PROJECT.replace("default_agent = 'custom'", "default_profile = \"omo\"")
    );
    assert_eq!(
        doc.config().unwrap().projects["my.app"]
            .default_profile
            .as_deref(),
        Some("omo")
    );
}

#[test]
fn semantic_profile_clear_removes_a_raw_alias_and_keeps_its_comments() {
    // Given a legacy field with both leading and inline comments.
    let mut doc = ConfigDoc::parse(LEGACY_PROJECT).unwrap();
    let base = doc.config().unwrap();
    let mut draft = base.clone();
    draft.projects.get_mut("my.app").unwrap().default_profile = None;
    // When the effective field is cleared.
    doc.apply_config_changes(&base, &draft).unwrap();
    // Then neither spelling remains, but comments and unrelated raw content survive.
    assert!(
        doc.get("projects.\"my.app\".default_agent")
            .unwrap()
            .is_none()
    );
    assert!(
        doc.get("projects.\"my.app\".default_profile")
            .unwrap()
            .is_none()
    );
    assert!(
        doc.config().unwrap().projects["my.app"]
            .default_profile
            .is_none()
    );
    let text = doc.to_string();
    for kept in [
        "# workspace notes",
        "path = '.' # repository",
        "# profile choice",
        "# legacy note",
        "base = 'concurrent' # branch notes",
        "owner_hint = 'raw-only'",
    ] {
        assert!(text.contains(kept), "{text}");
    }
}

#[test]
fn cleared_effective_arrays_replace_inherited_values_without_losing_siblings() {
    let base = ConfigDoc::parse(
        "[harnesses.omo]\ncommand = \"omo {model_args}\"\nefforts = [\"low\"]\n[profiles.omo]\nharness = \"omo\"\nargs = [\"--personal\"]\n",
    )
    .unwrap()
    .config()
    .unwrap();
    let mut draft = base.clone();
    draft.profiles.get_mut("omo").unwrap().args.clear();
    draft.harnesses.get_mut("omo").unwrap().efforts.clear();
    let mut workspace = ConfigDoc::parse(
        "# workspace notes\n[profiles.omo]\nharness = \"omo\" # retained\nmodel = \"concurrent\"\n",
    )
    .unwrap();

    workspace.apply_config_changes(&base, &draft).unwrap();

    for key in ["profiles.omo.args", "harnesses.omo.efforts"] {
        let item = workspace.get(key).unwrap().unwrap();
        assert!(item.as_array().unwrap().is_empty(), "{key}: {item}");
    }
    let text = workspace.to_string();
    assert!(text.contains("# workspace notes"));
    assert!(text.contains("# retained"));
    assert_eq!(
        workspace
            .get("profiles.omo.model")
            .unwrap()
            .unwrap()
            .as_str(),
        Some("concurrent")
    );
}

#[test]
fn effective_values_preserve_skipped_configuration_arrays_without_changing_pretty_output() {
    let config = ConfigDoc::parse(&format!(
        "{COMMENTED}\n[pricing.model]\ninput = 1.0\noutput = 2.0\n[[routing.rules]]\nprofile = \"omo\"\n"
    ))
    .unwrap()
    .config()
    .unwrap();
    let value = config.effective_value().unwrap();
    for (section, name, field) in [
        ("profiles", "omo", "args"),
        ("harnesses", "omo", "efforts"),
        ("projects", "app", "post_land"),
        ("pricing", "model", "aliases"),
    ] {
        assert!(value[section][name][field].as_array().unwrap().is_empty());
    }
    for field in ["project", "kind", "size"] {
        assert!(
            value["routing"]["rules"][0][field]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    let pretty: toml::Value = toml::from_str(&config.to_toml()).unwrap();
    assert!(pretty["profiles"]["omo"].get("args").is_none());
    assert!(pretty["harnesses"]["omo"].get("efforts").is_none());
}

#[test]
fn edits_keep_every_comment_and_unrelated_byte() {
    let (_d, s) = workspace(COMMENTED);
    let saved = s
        .edit_config(|doc| {
            doc.set_str("limits.running", "5")?;
            doc.set_str("profiles.deep.effort", "max")
        })
        .unwrap();
    assert_eq!(saved.limits.running, 5);
    let expected = COMMENTED
        .replace(
            "running = 3   # global slots",
            "running = 5   # global slots",
        )
        .replace(
            "effort = \"high\"     # claude levels",
            "effort = \"max\"     # claude levels",
        );
    assert_eq!(on_disk(&s), expected);

    s.edit_config(|doc| {
        let mut fast = Table::new();
        fast.insert("harness", value("claude"));
        fast.insert("model", value("sonnet"));
        doc.add_entry("profiles", "fast", fast)?;
        doc.set_str("limits.per_project.app", "1")?;
        doc.set_str("defaults.profile", "fast")?;
        assert!(doc.unset("projects.app.default_profile")?);
        Ok(())
    })
    .unwrap();
    let text = on_disk(&s);
    for line in COMMENTED.lines().filter(|l| l.contains('#')) {
        let line = line
            .replace("running = 3", "running = 5")
            .replace("\"high\"", "\"max\"");
        assert!(text.contains(&line), "lost {line:?} in:\n{text}");
    }
    assert!(text.contains("per_project = { app = 1 }"), "{text}");
    assert!(!text.contains("default_profile"), "{text}");
    let c = ConfigDoc::parse(&text).unwrap().config().unwrap();
    assert_eq!(c.profiles["fast"].model.as_deref(), Some("sonnet"));
    assert_eq!(c.defaults.profile.as_deref(), Some("fast"));
}

#[test]
fn invalid_edits_are_rejected_and_leave_the_file_alone() {
    let (_d, s) = workspace(COMMENTED);
    let err = s
        .edit_config(|doc| {
            doc.set_str("limits.running", "0")?;
            doc.set_str("profiles.deep.effort", "turbo")?;
            doc.set_str("projects.app.default_profile", "nobody")?;
            doc.set_str("projects.ghost.path", "missing-dir")
        })
        .unwrap_err();
    let p = problems(&err);
    assert_eq!(p.len(), 4, "{p:#?}");
    assert!(
        p.iter().any(|m| m == "limits.running must be > 0"),
        "{p:#?}"
    );
    assert!(
        p.iter().any(|m| m.contains("unknown effort 'turbo'")),
        "{p:#?}"
    );
    assert!(
        p.iter()
            .any(|m| m.starts_with("projects.app.default_profile: unknown profile 'nobody'")),
        "{p:#?}"
    );
    assert!(
        p.iter()
            .any(|m| m.starts_with("projects.ghost.path:") && m.ends_with("is not a directory")),
        "{p:#?}"
    );
    assert_eq!(on_disk(&s), COMMENTED);

    let err = s
        .edit_config(|doc| doc.set_str("limits.running", "lots"))
        .unwrap_err();
    assert!(format!("{err:#}").contains("refusing to save"), "{err:#}");
    assert!(s.edit_config(|doc| doc.set_str("limits", "1")).is_err());
    assert_eq!(on_disk(&s), COMMENTED);
}

#[test]
fn validate_reports_every_problem() {
    let bad = r#"
[limits]
running = 1
per_project = { nowhere = 0 }

[projects.app]
path = "app"

[harnesses.x]
command = ""
flags = "bogus"

[profiles.p]
harness = "missing"

[profiles.omo]
harness = "x"

[artifacts]
globs = ["/abs/**"]
max_bytes = 0

[review]
reviewer = "ghost"
"#;
    let d = tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("app")).unwrap();
    let c = ConfigDoc::parse(bad).unwrap().config().unwrap();
    let errs = c.validate(d.path()).unwrap_err();
    let text = errs.to_string();
    for want in [
        "limits.per_project.nowhere: unknown project 'nowhere'",
        "limits.per_project.nowhere must be > 0",
        "harnesses.x.command is empty",
        "harnesses.x: harness 'x': unknown flags 'bogus'",
        "profiles.p: profile 'p': unknown harness 'missing'",
        "artifacts.globs: glob \"/abs/**\" must be relative",
        "artifacts.max_bytes must be > 0",
        "[review]: unknown reviewer profile 'ghost'",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(
        text.starts_with(&format!("invalid config ({} problems):", errs.0.len())),
        "{text}"
    );
}

#[test]
fn save_config_rewrites_only_changed_keys() {
    let (_d, mut s) = workspace(COMMENTED);
    s.config.limits.running = 7;
    s.save_config().unwrap();
    let text = on_disk(&s);
    assert!(text.contains("running = 7   # global slots"), "{text}");
    for line in COMMENTED
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
    {
        assert!(text.contains(line), "lost {line:?} in:\n{text}");
    }
    assert_eq!(Store::open(s.root()).unwrap().config, s.config);
}

#[test]
fn reload_picks_up_external_edits_once() {
    let (_d, mut s) = workspace(COMMENTED);
    assert!(!s.reload_config_if_changed().unwrap());
    let other = Store::open(s.root()).unwrap();
    other
        .edit_config(|doc| doc.set_str("limits.running", "12"))
        .unwrap();
    assert!(s.reload_config_if_changed().unwrap());
    assert_eq!(s.config.limits.running, 12);
    assert!(!s.reload_config_if_changed().unwrap());
}

#[test]
fn get_reads_dotted_and_quoted_keys() {
    let mut doc = ConfigDoc::parse(COMMENTED).unwrap();
    doc.set_str("projects.\"my app\".path", "app").unwrap();
    let item = doc.get("projects.\"my app\".path").unwrap().unwrap();
    assert_eq!(item.as_str(), Some("app"));
    assert!(doc.get("projects.nope.path").unwrap().is_none());
    assert!(doc.get("limits.running.x").is_err());
}

use std::path::Path;

use corral_core::{
    Accept, Config, MergeMode, PolicySource, ReviewPolicy, Reviewer, Status, Store, validate_glob,
};

fn config(toml_text: &str) -> Config {
    toml::from_str(toml_text).unwrap()
}

#[test]
fn default_policy_is_the_manual_flow() {
    let p = ReviewPolicy::default();
    assert!(p.gate.is_empty());
    assert_eq!(p.reviewer, Reviewer::None);
    assert_eq!(p.accept, Accept::Owner);
    assert_eq!(p.max_rounds, 2);
    assert_eq!(p.merge, MergeMode::Rebase);
    assert!(p.remove_worktree);
    assert_eq!(p.max_diff_lines, 1500);
    assert!(p.owner_paths.is_empty());

    let c = Config::default_for(Path::new("/nonexistent"));
    assert_eq!(c.review_policy(None, None), ReviewPolicy::default());
    let serialized: toml::Table = c.to_toml().parse().unwrap();
    assert!(!serialized.contains_key("review"), "{}", c.to_toml());
    c.validate_review().unwrap();
}

#[test]
fn policy_resolves_global_then_project_then_task() {
    let c = config(
        r#"
        [review]
        gate = ["cargo test"]
        reviewer = "pm"
        accept = "auto"
        max_rounds = 3

        [projects.app]
        path = "app"
        [projects.app.review]
        gate = ["just check"]
        accept = "owner"
        merge = "ff"
        owner_paths = [".github/**"]

        [projects.lib]
        path = "lib"
        "#,
    );
    let global = c.review_policy(Some("lib"), None);
    assert_eq!(global.gate, vec!["cargo test"]);
    assert_eq!(global.reviewer, Reviewer::Pm);
    assert_eq!(global.accept, Accept::Auto);
    assert_eq!(global.max_rounds, 3);
    assert_eq!(global.merge, MergeMode::Rebase);

    let project = c.review_policy(Some("app"), None);
    assert_eq!(project.gate, vec!["just check"]);
    assert_eq!(project.reviewer, Reviewer::Pm);
    assert_eq!(project.accept, Accept::Owner);
    assert_eq!(project.max_rounds, 3);
    assert_eq!(project.merge, MergeMode::Ff);
    assert_eq!(project.owner_paths, vec![".github/**"]);

    let task = c.review_policy(Some("app"), Some(Accept::Auto));
    assert_eq!(task.accept, Accept::Auto);
    assert_eq!(task.gate, vec!["just check"]);
    assert_eq!(c.review_policy(None, None).gate, vec!["cargo test"]);

    let s = c.review_policy_sources(Some("app"), Some(Accept::Auto));
    assert_eq!(s.gate, PolicySource::Project);
    assert_eq!(s.reviewer, PolicySource::Global);
    assert_eq!(s.accept, PolicySource::Task);
    assert_eq!(s.max_rounds, PolicySource::Global);
    assert_eq!(s.merge, PolicySource::Project);
    assert_eq!(s.max_diff_lines, PolicySource::Default);
    assert_eq!(s.owner_paths, PolicySource::Project);
    assert_eq!(s.autopilot, PolicySource::Default);
    let s = c.review_policy_sources(Some("lib"), None);
    assert_eq!(
        (s.gate, s.accept, s.merge),
        (
            PolicySource::Global,
            PolicySource::Global,
            PolicySource::Default
        )
    );
    assert_eq!(
        PolicySource::Project.label(Some("app")),
        "[projects.app.review]"
    );
}

#[test]
fn review_config_round_trips_through_toml() {
    let c = config(
        r#"
        [review]
        gate = ["true"]
        [projects.app]
        path = "app"
        [projects.app.review]
        accept = "auto"
        "#,
    );
    let again: Config = toml::from_str(&c.to_toml()).unwrap();
    assert_eq!(again, c);
}

#[test]
fn validation_rejects_unknown_reviewers_bad_globs_and_empty_commands() {
    let c = config(
        r#"
        [profiles.deep]
        harness = "claude"

        [review]
        reviewer = "nobody"
        owner_paths = ["/abs/**", "src/a**b", "ok/**/*.toml"]

        [projects.app]
        path = "app"
        [projects.app.review]
        reviewer = "deep"
        gate = ["  "]
        owner_paths = ["x/{a,b}"]
        "#,
    );
    let err = format!("{:#}", c.validate_review().unwrap_err());
    assert!(
        err.contains("[review]: unknown reviewer profile 'nobody'"),
        "{err}"
    );
    assert!(err.contains("\"/abs/**\" must be relative"), "{err}");
    assert!(err.contains("'**' must be a whole path segment"), "{err}");
    assert!(!err.contains("ok/**/*.toml"), "{err}");
    assert!(!err.contains("'deep'"), "{err}");
    assert!(
        err.contains("[projects.app.review]: gate has an empty command"),
        "{err}"
    );
    assert!(err.contains("braces are not supported"), "{err}");

    let ok = config(
        r#"
        [profiles.deep]
        harness = "claude"
        [review]
        reviewer = "deep"
        owner_paths = [".github/**", "**/Cargo.toml"]
        "#,
    );
    ok.validate_review().unwrap();
    for bad in ["", "a//b", "../x", "a/[ab]"] {
        assert!(validate_glob(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn autopilot_switch_defaults_on_and_is_workspace_wide() {
    assert!(config("").autopilot_enabled());
    assert!(!config("[review]\nautopilot = false\n").autopilot_enabled());
    let per_project =
        config("[projects.app]\npath = \"app\"\n[projects.app.review]\nautopilot = false\n");
    let err = format!("{:#}", per_project.validate_review().unwrap_err());
    assert!(
        err.contains("[projects.app.review]: autopilot is workspace-wide"),
        "{err}"
    );
}

#[test]
fn store_set_autopilot_edits_corral_toml_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    let file = store.dir().join("corral.toml");
    let text = format!(
        "# keep me\n{}\n[review] # policy\naccept = \"owner\" # the owner lands\n",
        std::fs::read_to_string(&file).unwrap()
    );
    std::fs::write(&file, &text).unwrap();

    store.set_autopilot(false).unwrap();
    assert!(!store.config.autopilot_enabled());
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(
        saved.starts_with("# keep me\n")
            && saved.contains(
                "[review] # policy\naccept = \"owner\" # the owner lands\nautopilot = false\n"
            ),
        "{saved}"
    );
    assert!(!Store::open(tmp.path()).unwrap().config.autopilot_enabled());

    store.set_autopilot(true).unwrap();
    assert!(Store::open(tmp.path()).unwrap().config.autopilot_enabled());
}

#[test]
fn unknown_review_keys_are_parse_errors() {
    let err = toml::from_str::<Config>("[review]\naccpet = \"auto\"\n").unwrap_err();
    assert!(err.to_string().contains("accpet"), "{err}");
    assert!(toml::from_str::<Config>("[review]\naccept = \"maybe\"\n").is_err());
}

#[test]
fn task_review_frontmatter_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let t = store.create("Auto task", None, Status::Ready, "").unwrap();
    assert_eq!(t.review, None);
    let text = std::fs::read_to_string(&t.path).unwrap();
    assert!(!text.contains("review:"), "{text}");
    store
        .update(&t.id, |t| {
            t.review = Some(Accept::Auto);
            Ok(())
        })
        .unwrap();
    let text = std::fs::read_to_string(&t.path).unwrap();
    assert!(text.contains("review: auto"), "{text}");
    assert_eq!(store.get(&t.id).unwrap().review, Some(Accept::Auto));
}

use corral_core::{Config, ConfigDoc, DispatchPolicy, PromoteIdeas, Store};
use tempfile::tempdir;

#[test]
fn policy_defaults_when_dispatch_fields_are_omitted() {
    // Given both an old config and a workspace with no discovered projects.
    let root = tempdir().unwrap();
    for config in [
        toml::from_str::<Config>("").unwrap(),
        Config::default_for(root.path()),
        toml::from_str::<Config>("[dispatch]\npaused = true\nready_projects = []").unwrap(),
    ] {
        // When effective settings are resolved without a project.
        let policy = config.dispatch_policy(None);
        // Then legacy configs receive the intended selection defaults.
        assert_eq!(
            policy,
            DispatchPolicy {
                auto_backlog: true,
                promote_ideas: PromoteIdeas::Suggest,
                max_auto_promotions_per_day: 3,
            }
        );
    }
}

#[test]
fn partial_project_overrides_inherit_nondefault_global_fields() {
    // Given distinct global values and one override for each project.
    let config: Config = toml::from_str(
        r#"
[dispatch]
paused = true
ready_projects = ["backlog"]
auto_backlog = false
promote_ideas = "auto"
max_auto_promotions_per_day = 8
[projects.backlog]
path = "."
[projects.backlog.dispatch]
auto_backlog = true
[projects.ideas]
path = "."
[projects.ideas.dispatch]
promote_ideas = "never"
[projects.budget]
path = "."
[projects.budget.dispatch]
max_auto_promotions_per_day = 2
[projects.inherited]
path = "."
"#,
    )
    .unwrap();
    let global = DispatchPolicy {
        auto_backlog: false,
        promote_ideas: PromoteIdeas::Auto,
        max_auto_promotions_per_day: 8,
    };
    for (project, expected) in [
        (None, global),
        (Some("unknown"), global),
        (Some("inherited"), global),
        (
            Some("backlog"),
            DispatchPolicy {
                auto_backlog: true,
                ..global
            },
        ),
        (
            Some("ideas"),
            DispatchPolicy {
                promote_ideas: PromoteIdeas::Never,
                ..global
            },
        ),
        (
            Some("budget"),
            DispatchPolicy {
                max_auto_promotions_per_day: 2,
                ..global
            },
        ),
    ] {
        // When settings are resolved for that scope.
        let policy = config.dispatch_policy(project);
        // Then only explicitly overridden fields differ from the global policy.
        assert_eq!(policy, expected, "{project:?}");
    }
    assert!(config.dispatch.paused);
    assert_eq!(config.dispatch.ready_projects, ["backlog"]);
}

#[test]
fn explicit_false_and_suggest_overrides_survive_save_and_reload() {
    // Given nondefault globals and project settings that differ from them.
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    // When config editing saves both layers through schema validation.
    store
        .edit_config(|doc| {
            doc.set_str("dispatch.promote_ideas", "auto")?;
            doc.set_str("dispatch.max_auto_promotions_per_day", "9")?;
            doc.set_str("projects.app.path", ".")?;
            doc.set_str("projects.app.dispatch.auto_backlog", "false")?;
            doc.set_str("projects.app.dispatch.promote_ideas", "suggest")
        })
        .unwrap();
    // Then reloading preserves explicit false/suggest and inherits the global budget.
    let config = Config::load(&store.dir().join("corral.toml")).unwrap();
    assert_eq!(
        config.dispatch_policy(Some("app")),
        DispatchPolicy {
            auto_backlog: false,
            promote_ideas: PromoteIdeas::Suggest,
            max_auto_promotions_per_day: 9,
        }
    );
    let round_trip: Config = toml::from_str(&config.to_toml()).unwrap();
    assert_eq!(round_trip, config);
}

#[test]
fn config_schema_rejects_invalid_dispatch_values_in_both_scopes() {
    // Given malformed enum, boolean, and unsigned integer values.
    for (key, value) in [
        ("promote_ideas", "\"sometimes\""),
        ("promote_ideas", "\"AUTO\""),
        ("auto_backlog", "\"true\""),
        ("max_auto_promotions_per_day", "-1"),
        ("max_auto_promotions_per_day", "1.5"),
        ("max_auto_promotions_per_day", "\"3\""),
        ("max_auto_promotions_per_day", "9223372036854775808"),
    ] {
        for scope in ["dispatch", "projects.app.dispatch"] {
            let mut doc = ConfigDoc::parse("[projects.app]\npath = '.'\n").unwrap();
            doc.set_str(&format!("{scope}.{key}"), value).unwrap();
            // When the same schema used by config check decodes the document.
            let result = doc.config();
            // Then invalid values cannot enter typed configuration.
            assert!(result.is_err(), "{scope}.{key} = {value}");
        }
    }
}

#[test]
fn config_check_reports_zero_budgets_in_both_scopes() {
    // Given a valid workspace with zero global and per-project budgets.
    let root = tempdir().unwrap();
    let mut config = Config::default_for(root.path());
    let mut doc = ConfigDoc::parse(&config.to_toml()).unwrap();
    doc.set_str("dispatch.max_auto_promotions_per_day", "0")
        .unwrap();
    doc.set_str("projects.app.path", ".").unwrap();
    doc.set_str("projects.app.dispatch.max_auto_promotions_per_day", "0")
        .unwrap();
    config = doc.config().unwrap();
    // When config check applies semantic validation.
    let errors = config.validate(root.path()).unwrap_err().0;
    // Then both invalid limits are reported together.
    assert_eq!(
        errors,
        [
            "dispatch.max_auto_promotions_per_day must be > 0",
            "projects.app.dispatch.max_auto_promotions_per_day must be > 0",
        ]
    );
}

#[test]
fn invalid_dispatch_edit_leaves_saved_config_unchanged() {
    // Given an initialized workspace.
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let before = Config::load(&store.dir().join("corral.toml")).unwrap();
    // When an invalid project promotion budget is submitted.
    let result = store.edit_config(|doc| {
        doc.set_str("projects.app.path", ".")?;
        doc.set_str("projects.app.dispatch.max_auto_promotions_per_day", "0")
    });
    // Then validation refuses the write.
    assert!(result.is_err());
    assert_eq!(
        Config::load(&store.dir().join("corral.toml")).unwrap(),
        before
    );
}

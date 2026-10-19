use corral_core::{Config, ProfileChoice, RouteSource, Status, Store, Task, TaskKind, TaskSize};

fn fixture(rules: &str) -> (tempfile::TempDir, Config, Task) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("app/.git")).unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut config = store.config.clone();
    let profiles: Config = toml::from_str(
        r#"
[profiles.first]
harness = "omo"
model = "vendor/first"
effort = "high"
[profiles.second]
harness = "omo"
model = "vendor/second"
effort = "low"
"#,
    )
    .unwrap();
    config.profiles.extend(profiles.profiles);
    let parsed: Config = toml::from_str(rules).unwrap();
    config.routing = parsed.routing;
    let mut task = store
        .create(
            "Parser UI",
            Some("app"),
            Status::Ready,
            "Acceptance: atomic",
        )
        .unwrap();
    task.kind = Some(TaskKind::Ui.to_string());
    task.size = Some(TaskSize::M);
    (dir, config, task)
}

#[test]
fn rule_matches_all_fields_and_alternatives_in_order() {
    // Given multiple matching rules with scalar and list fields.
    let (_dir, config, task) = fixture(
        r#"
[[routing.rules]]
project = "other"
profile = "second"
[[routing.rules]]
name = "UI parser"
project = ["app", "other"]
kind = ["ui", "research"]
size = "m"
title_regex = "(?i)^parser"
body_regex = "atomic$"
profile = "first"
[[routing.rules]]
profile = "second"
"#,
    );
    // When resolving a task.
    let decision = config
        .route_task(&task, &ProfileChoice::default(), |_| None)
        .unwrap();
    // Then the first fully matching rule wins.
    assert_eq!(decision.profile, "first");
    assert_eq!(
        decision.route,
        RouteSource::Rule {
            index: 2,
            name: Some("UI parser".into())
        }
    );
    assert_eq!(decision.model.as_deref(), Some("vendor/first"));
    assert_eq!(decision.effort.as_deref(), Some("high"));
}

#[test]
fn each_constraint_rejects_nonmatching_or_missing_hints() {
    let (_dir, config, task) = fixture(
        r#"
[[routing.rules]]
project = "app"
kind = "ui"
size = "m"
title = "Parser"
body = "atomic"
profile = "first"
"#,
    );
    let rule = &config.routing.rules[0];
    for field in ["project", "kind", "size", "title", "body"] {
        // Given one constraint that does not match.
        let mut task = task.clone();
        match field {
            "project" => task.project = None,
            "kind" => task.kind = None,
            "size" => task.size = Some(TaskSize::Xl),
            "title" => task.title = "Different".into(),
            "body" => task.body.clear(),
            _ => unreachable!(),
        }
        // When evaluating the rule, then it must not match.
        assert!(!rule.matches(&task).unwrap(), "{field}");
    }
}

#[test]
fn resolution_precedence_covers_every_layer() {
    // Given every profile source, removing higher priority sources in turn.
    let (_dir, config, task) = fixture("[[routing.rules]]\nprofile = 'first'\n");
    for (override_name, task_name, rules, project, defaults, expected) in [
        (
            Some("second"),
            Some("first"),
            true,
            true,
            true,
            RouteSource::Override,
        ),
        (None, Some("second"), true, true, true, RouteSource::Task),
        (
            None,
            None,
            true,
            true,
            true,
            RouteSource::Rule {
                index: 1,
                name: None,
            },
        ),
        (None, None, false, true, true, RouteSource::ProjectDefault),
        (None, None, false, false, true, RouteSource::Defaults),
        (None, None, false, false, false, RouteSource::Fallback),
    ] {
        let mut config = config.clone();
        let mut task = task.clone();
        task.profile = task_name.map(str::to_string);
        if !rules {
            config.routing.rules.clear();
        }
        config.projects.get_mut("app").unwrap().default_profile = project.then(|| "second".into());
        config.defaults.profile = defaults.then(|| "second".into());
        let choice = ProfileChoice {
            profile: override_name.map(str::to_string),
            ..Default::default()
        };
        // When routing, then the highest remaining source wins.
        let decision = config.route_task(&task, &choice, |_| None).unwrap();
        assert_eq!(decision.route, expected);
        assert_eq!(
            decision.profile,
            if matches!(expected, RouteSource::Rule { .. }) {
                "first"
            } else if expected == RouteSource::Fallback {
                "omo"
            } else {
                "second"
            }
        );
    }
}

#[test]
fn unavailable_profiles_fall_through_to_next_rule_and_defaults() {
    // Given an unavailable first rule, without a manual pin.
    let (_dir, mut config, task) =
        fixture("[[routing.rules]]\nprofile = 'first'\n[[routing.rules]]\nprofile = 'second'\n");
    config.defaults.profile = Some("second".into());
    // When availability rejects the first profile.
    let decision = config
        .route_task(&task, &ProfileChoice::default(), |p| {
            (p.profile == "first").then(|| "model not listed".into())
        })
        .unwrap();
    // Then automatic rule routing continues and records the rejected candidate.
    assert_eq!(decision.profile, "second");
    assert_eq!(
        decision.route,
        RouteSource::Rule {
            index: 2,
            name: None
        }
    );
    assert_eq!(decision.warnings.len(), 1);
}

#[test]
fn unavailable_task_pin_cannot_become_another_profile() {
    // Given an unavailable pin above an available owner rule.
    let (_dir, config, mut task) = fixture("[[routing.rules]]\nprofile = 'second'\n");
    task.profile = Some("first".into());
    // When routing the pinned task.
    let result = config.route_task(&task, &ProfileChoice::default(), |profile| {
        (profile.profile == "first").then(|| "unavailable model".into())
    });
    // Then dispatch cannot silently substitute the available rule.
    assert!(result.is_err());
    assert_eq!(task.profile.as_deref(), Some("first"));
}

#[test]
fn unavailable_rules_and_project_default_reach_workspace_default() {
    let (_dir, mut config, task) = fixture("[[routing.rules]]\nprofile = 'first'\n");
    config.projects.get_mut("app").unwrap().default_profile = Some("first".into());
    config.defaults.profile = Some("second".into());
    let decision = config
        .route_task(&task, &ProfileChoice::default(), |p| {
            (p.profile == "first").then(|| "model not listed".into())
        })
        .unwrap();
    assert_eq!(decision.profile, "second");
    assert_eq!(decision.route, RouteSource::Defaults);
    assert_eq!(decision.warnings.len(), 2);
}

#[test]
fn explicit_model_and_effort_override_rule_preset() {
    let (_dir, config, task) = fixture("[[routing.rules]]\nprofile = 'first'\n");
    let choice = ProfileChoice {
        model: Some("vendor/manual".into()),
        effort: Some("MAX".into()),
        ..Default::default()
    };
    let decision = config
        .route_task(&task, &choice, |_| Some("model not listed".into()))
        .unwrap();
    assert_eq!(decision.profile, "first");
    assert_eq!(decision.model.as_deref(), Some("vendor/manual"));
    assert_eq!(decision.effort.as_deref(), Some("max"));
    assert_eq!(decision.route, RouteSource::Override);
    assert_eq!(decision.warnings.len(), 1);
}

#[test]
fn first_available_match_does_not_evaluate_later_rules() {
    // Given a usable first match and an invalid regex on a later rule.
    let (_dir, config, task) = fixture(
        "[[routing.rules]]\nprofile = 'first'\n[[routing.rules]]\ntitle_regex = '['\nprofile = 'second'\n",
    );
    // When routing, then the later rule does not affect the selected profile.
    let decision = config
        .route_task(&task, &ProfileChoice::default(), |_| None)
        .unwrap();
    assert_eq!(decision.profile, "first");
}

#[test]
fn bad_regex_is_an_actionable_error() {
    let (_dir, config, task) = fixture("[[routing.rules]]\ntitle_regex = '['\nprofile = 'first'\n");
    let err = config
        .route_task(&task, &ProfileChoice::default(), |_| None)
        .unwrap_err();
    assert!(format!("{err:#}").contains("routing rule 1: invalid routing title regex"));
}

#[test]
fn hints_routes_and_config_round_trip_without_changing_legacy_tasks() {
    let (dir, config, mut task) = fixture("[[routing.rules]]\nkind = 'ui'\nprofile = 'first'\n");
    let store = Store::open(dir.path()).unwrap();
    store.save(&task).unwrap();
    let saved = store.get(&task.id).unwrap();
    assert_eq!((saved.kind, saved.size), (task.kind, task.size));
    let decoded: Config = toml::from_str(&config.to_toml()).unwrap();
    assert_eq!(decoded.routing, config.routing);
    task.kind = None;
    task.size = None;
    store.save(&task).unwrap();
    assert!(
        !std::fs::read_to_string(task.path)
            .unwrap()
            .contains("kind:")
    );
}

#[test]
fn starter_rules_can_be_enabled_as_valid_config() {
    let rules = include_str!("../src/routing-starter.toml");
    let text = rules
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .skip_while(|line| !line.starts_with("[profiles."))
        .collect::<Vec<_>>()
        .join("\n");
    let config: Config = toml::from_str(&text).unwrap();
    assert_eq!(config.routing.rules.len(), 9);
    assert_eq!(config.profiles.len(), 5);
    assert_eq!(config.defaults.profile.as_deref(), Some("gpt-sol"));

    // Given: routine tasks and the enabled starter, including its optional Fast profile.
    let (_dir, mut enabled, mut task) = fixture("");
    enabled.profiles.extend(config.profiles);
    enabled.routing = config.routing;
    enabled.defaults = config.defaults;
    for (kind, size, profile, model) in [
        ("backend", TaskSize::Xs, "gpt-sol", "gpt-6.1-sol"),
        ("backend", TaskSize::S, "gpt-sol", "gpt-6.1-sol"),
        ("test", TaskSize::M, "gpt-sol", "gpt-6.1-sol"),
        ("docs", TaskSize::Xs, "gpt-luna", "gpt-6-luna-fast"),
        ("docs", TaskSize::M, "gpt-luna", "gpt-6-luna-fast"),
    ] {
        task.kind = Some(kind.into());
        task.size = Some(size);
        // When: all configured models are available.
        let decision = enabled
            .route_task(&task, &ProfileChoice::default(), |_| None)
            .unwrap();
        // Then: the matching rule chooses Sol or Luna, never the Sol Fast option.
        assert_eq!(decision.profile, profile, "{kind}/{size}");
        assert_eq!(
            decision.model.as_deref(),
            Some(format!("chatgpt-subscription/{model}").as_str()),
            "{kind}/{size}"
        );
        assert!(matches!(decision.route, RouteSource::Rule { .. }));
    }
}

use std::path::Path;

use corral_core::{Config, Reviewer, Task, parse_task};

const OWNER: &str = r#"
[review]
reviewer = "gpt-sol"

[agents.gpt-sol]
command = "true"

[agents.gpt-sol-med]
command = "true"

[projects.app]
path = "."

[projects.pinned]
path = "."

[projects.pinned.review]
reviewer = "gpt-sol"

[[routing.reviewer_rules]]
name = "small"
size = ["xs", "s"]
profile = "gpt-sol-med"
"#;

fn task(project: &str, size: Option<&str>, extra: &str) -> Task {
    let size = size.map(|s| format!("size: {s}\n")).unwrap_or_default();
    let text = format!(
        "---\nid: T-001\ntitle: Ship the widget\nstatus: ready\ncreated: 2026-10-20T00:00:00Z\nupdated: 2026-10-20T00:00:00Z\nproject: {project}\n{size}{extra}---\nBody.\n"
    );
    parse_task(&text, Path::new("T-001.md")).unwrap()
}

fn reviewer(config: &Config, task: &Task) -> Reviewer {
    config.task_review_policy(task).reviewer
}

#[test]
fn first_matching_reviewer_rule_replaces_workspace_reviewer() {
    // Given the owner's split: s/xs -> gpt-sol-med, everything else -> [review] gpt-sol.
    let config: Config = toml::from_str(OWNER).unwrap();
    config.validate_review().unwrap();
    for (size, expected, rule) in [
        (Some("xs"), "gpt-sol-med", Some(1)),
        (Some("s"), "gpt-sol-med", Some(1)),
        (Some("m"), "gpt-sol", None),
        (Some("xl"), "gpt-sol", None),
        (None, "gpt-sol", None),
    ] {
        let task = task("app", size, "");
        // Then the first matching rule wins and unmatched tasks fall back to [review].
        assert_eq!(
            reviewer(&config, &task),
            Reviewer::Profile(expected.into()),
            "{size:?}"
        );
        assert_eq!(
            config.reviewer_rule(&task).map(|(i, _)| i),
            rule,
            "{size:?}"
        );
    }
}

#[test]
fn rules_are_ordered_and_use_the_routing_matcher() {
    let mut config: Config = toml::from_str(OWNER).unwrap();
    config.routing.reviewer_rules.insert(
        0,
        toml::from_str("title = '(?i)widget'\nkind = 'backend'\nprofile = 'gpt-sol'").unwrap(),
    );
    // A matching earlier rule wins over the size rule.
    let backend = task("app", Some("s"), "kind: backend\n");
    assert_eq!(
        reviewer(&config, &backend),
        Reviewer::Profile("gpt-sol".into())
    );
    assert_eq!(config.reviewer_rule(&backend).map(|(i, _)| i), Some(1));
    // Every set field must match: a ui task skips rule 1 and reaches rule 2.
    let ui = task("app", Some("s"), "kind: ui\n");
    assert_eq!(
        reviewer(&config, &ui),
        Reviewer::Profile("gpt-sol-med".into())
    );
    assert_eq!(config.reviewer_rule(&ui).map(|(i, _)| i), Some(2));
}

#[test]
fn task_and_project_reviewers_keep_precedence_over_rules() {
    let config: Config = toml::from_str(OWNER).unwrap();
    // An explicit project reviewer keeps its meaning.
    let pinned = task("pinned", Some("xs"), "");
    assert_eq!(
        reviewer(&config, &pinned),
        Reviewer::Profile("gpt-sol".into())
    );
    assert!(config.reviewer_rule(&pinned).is_none());
    // So does the task's own reviewer override.
    let own = task("app", Some("xs"), "reviewer: pm\n");
    assert_eq!(reviewer(&config, &own), Reviewer::Pm);
    assert!(config.reviewer_rule(&own).is_none());
}

#[test]
fn require_cross_family_survives_rule_selection() {
    let mut config: Config = toml::from_str(OWNER).unwrap();
    config.review.require_cross_family = Some(true);
    let small = task("app", Some("s"), "");
    assert!(config.task_review_policy(&small).cross_family_required());
    // A rule replacing a workspace cross-vendor selector keeps the diversity requirement.
    config.review.require_cross_family = None;
    config.review.reviewer = Some("cross-vendor".into());
    let policy = config.task_review_policy(&small);
    assert_eq!(policy.reviewer, Reviewer::Profile("gpt-sol-med".into()));
    assert!(policy.cross_family_required());
    assert!(
        policy
            .check_reviewer_family(Some("openai"), Some("openai"))
            .is_err()
    );
    // An unmatched task keeps the workspace cross-vendor selector itself.
    let large = task("app", Some("l"), "");
    assert_eq!(reviewer(&config, &large), Reviewer::CrossVendor);
}

#[test]
fn validation_rejects_unknown_rule_profiles_and_invalid_regexes() {
    let mut config: Config = toml::from_str(OWNER).unwrap();
    config.routing.reviewer_rules[0].profile = "gpt-sol-typo".into();
    let error = config.validate_review().unwrap_err().to_string();
    assert!(
        error.contains("routing.reviewer_rules[1]: unknown reviewer profile 'gpt-sol-typo'"),
        "{error}"
    );
    for keyword in ["pm", "none", "cross-vendor"] {
        config.routing.reviewer_rules[0].profile = keyword.into();
        config.validate_review().unwrap();
    }
    config.routing.reviewer_rules[0].title_regex = Some("(".into());
    let error = config.validate_review().unwrap_err().to_string();
    assert!(
        error.contains("routing.reviewer_rules[1]: invalid regex"),
        "{error}"
    );
    // Unknown keys are rejected as for routing.rules.
    assert!(
        toml::from_str::<Config>("[[routing.reviewer_rules]]\nprofile = 'a'\nsizes = 's'").is_err()
    );
}

#[test]
fn reviewer_rules_round_trip() {
    let config: Config = toml::from_str(OWNER).unwrap();
    let roundtrip: Config = toml::from_str(&config.to_toml()).unwrap();
    assert_eq!(roundtrip, config);
    assert!(!config.routing.is_empty());
}

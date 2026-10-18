use super::*;

#[test]
fn project_repository_path_is_relevant_but_base_branch_is_not() {
    // Given a configured project's setup identity.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .projects
        .insert("app".into(), toml::from_str("path = '.'").unwrap());
    let revision = config.profile_setup_revision().unwrap();
    // When its repository path changes.
    config.projects.get_mut("app").unwrap().path = "other-repository".into();
    // Then that eligibility context changes identity, unlike an unrelated base branch.
    assert_ne!(revision, config.profile_setup_revision().unwrap());
    config.projects.get_mut("app").unwrap().path = ".".into();
    config.projects.get_mut("app").unwrap().base = Some("other-base".into());
    assert_eq!(revision, config.profile_setup_revision().unwrap());
}

#[test]
fn revision_tracks_bindings_and_eligibility_but_excludes_unrelated_fields() {
    // Given a setup configuration identity.
    let dir = tempfile::tempdir().unwrap();
    let config = Config::default_for(dir.path());
    let revision = config.profile_setup_revision().unwrap();
    let mut unrelated = config.clone();
    unrelated.limits.running = 31;
    unrelated.pm.push_events = true;
    unrelated.artifacts.max_bytes = 42;
    unrelated.herdr.session = Some("other".into());
    // When changing only fields unrelated to setup.
    let unchanged = unrelated.profile_setup_revision().unwrap();
    // Then semantic setup identity is stable; binding/policy edits are not.
    assert_eq!(revision, unchanged);
    let mut relevant = config.clone();
    relevant.pm.agent = "claude".into();
    assert_ne!(revision, relevant.profile_setup_revision().unwrap());
    relevant = config.clone();
    relevant.review.accept = Some(Accept::Auto);
    assert_ne!(revision, relevant.profile_setup_revision().unwrap());
    relevant = config.clone();
    relevant.dispatch.ready_projects = vec!["restricted".into()];
    assert_ne!(revision, relevant.profile_setup_revision().unwrap());
    relevant = config.clone();
    relevant.profiles.insert("new".into(), profile("openai"));
    let with_profile = relevant.profile_setup_revision().unwrap();
    relevant
        .profiles
        .get_mut("new")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .eligibility
        .enabled = false;
    assert_ne!(with_profile, relevant.profile_setup_revision().unwrap());
}

#[test]
fn roles_merge_without_changing_acceptance_or_autopilot() {
    // Given existing roles and independent review controls.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .routing
        .role_profiles
        .insert(ProfileRole::Planning, "claude".into());
    config.review.accept = Some(Accept::Owner);
    config.review.autopilot = Some(false);
    let mut p = proposal(&config);
    p.roles.insert(ProfileRole::Pm, "custom".into());
    p.roles
        .insert(ProfileRole::ImplementationReview, "custom".into());
    // When applying explicit role choices.
    let after = config.with_profile_setup(&p).unwrap();
    // Then approved launch bindings change without replacing other roles or policy.
    assert_eq!(after.pm.agent, "custom");
    assert_eq!(after.review.reviewer.as_deref(), Some("custom"));
    assert_eq!(
        after.routing.role_profiles[&ProfileRole::Planning],
        "claude"
    );
    assert_eq!(after.review.accept, Some(Accept::Owner));
    assert_eq!(after.review.autopilot, Some(false));
    assert_eq!(after.automation, config.automation);
}

#[test]
fn cross_family_review_checks_effective_implementation_not_only_role_map() {
    // Given an existing OpenAI implementation binding omitted from the proposal roles.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.profiles.insert("author".into(), profile("openai"));
    config.defaults.profile = Some("author".into());
    config.review.require_cross_family = Some(true);
    let mut p = proposal(&config);
    p.roles.clear();
    p.roles
        .insert(ProfileRole::ImplementationReview, "custom".into());
    // When choosing a same-family reviewer.
    let result = config.with_profile_setup(&p);
    // Then an incomplete role map cannot bypass effective launcher independence.
    assert!(result.is_err());
}

#[test]
fn cross_family_review_checks_effective_planning_binding() {
    // Given an Anthropic implementer, OpenAI planner, and required diversity.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .profiles
        .insert("author".into(), profile("anthropic"));
    config.profiles.insert("planner".into(), profile("openai"));
    config.defaults.profile = Some("author".into());
    config
        .routing
        .role_profiles
        .insert(ProfileRole::Planning, "planner".into());
    config.review.require_cross_family = Some(true);
    let mut p = proposal(&config);
    p.roles.clear();
    p.roles
        .insert(ProfileRole::ImplementationReview, "custom".into());
    // When the reviewer differs from the implementer but not the planner.
    let result = config.with_profile_setup(&p);
    // Then planning independence is checked against the actual saved role too.
    assert!(result.is_err());
}

#[test]
fn cross_family_pm_reviewer_checks_the_configured_pm_binding() {
    // Given PM review whose PM has the author's family.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .profiles
        .insert("pm-profile".into(), profile("openai"));
    config.pm.agent = "pm-profile".into();
    config.review.reviewer = Some("pm".into());
    config.review.require_cross_family = Some(true);
    let p = proposal(&config);
    // When the setup changes the effective implementer to the PM's family.
    let result = config.with_profile_setup(&p);
    // Then PM review cannot bypass the existing family check.
    assert!(result.is_err());
}

#[test]
fn cross_vendor_selector_is_not_silently_disabled_by_explicit_reviewer_role() {
    // Given an existing cross-vendor selector and known independent mapping.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config
        .profiles
        .insert("reviewer".into(), profile("anthropic"));
    config.review.reviewer = Some("cross-vendor".into());
    config
        .routing
        .reviewer_by_vendor
        .insert("anthropic".into(), "reviewer".into());
    let mut p = proposal(&config);
    p.roles
        .insert(ProfileRole::ImplementationReview, "reviewer".into());
    // When assigning an approved reviewer role.
    let after = config.with_profile_setup(&p).unwrap();
    // Then the existing launch selector and its implied family requirement survive.
    assert_eq!(after.review.reviewer.as_deref(), Some("cross-vendor"));
    assert!(after.review_policy(None, None).cross_family_required());
    assert_eq!(
        after.routing.role_profiles[&ProfileRole::ImplementationReview],
        "reviewer"
    );
}

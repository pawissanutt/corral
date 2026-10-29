use super::*;

#[test]
fn presets_and_overrides_respect_role_ceiling() {
    // Given each preset and a per-role override different from its default.
    for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
        let mut config = AgentCapabilities::default();
        config.set_preset(preset);
        let enabled = preset != Preset::Strict;
        assert_eq!(config.todo(AgentRole::Worker), enabled);
        assert_eq!(config.todo(AgentRole::Pm), enabled);
        // When worker overrides its preset.
        config.set(AgentRole::Worker, "todo", !enabled).unwrap();
        // Then PM retains its default and foreign roles remain excluded.
        assert_eq!(config.todo(AgentRole::Worker), !enabled);
        assert_eq!(config.todo(AgentRole::Pm), enabled);
        for role in [
            AgentRole::Reviewer,
            AgentRole::Planner,
            AgentRole::PlanReviewer,
        ] {
            assert!(!config.todo(role));
        }
    }
}

#[test]
fn omitted_capabilities_default_to_standard_and_parse_role_disable() {
    // Given absent configuration and a configured worker opt-out.
    let absent: crate::config::AgentCfg = toml::from_str("").unwrap();
    let configured: crate::config::AgentCfg =
        toml::from_str("[capabilities.worker]\ntodo = false").unwrap();
    // Then absent config enables worker/PM, while only the worker opts out.
    assert!(absent.capabilities.todo(AgentRole::Worker));
    assert!(absent.capabilities.todo(AgentRole::Pm));
    assert!(!configured.capabilities.todo(AgentRole::Worker));
    assert!(configured.capabilities.todo(AgentRole::Pm));
}

#[test]
fn overrides_outside_the_catalog_or_role_ceiling_are_config_errors() {
    // Given an unknown key, a ceiling-disallowed key and every role table.
    let parse = |text: &str| toml::from_str::<crate::config::AgentCfg>(text).map(|_| ());
    // Then parsing names the table and the reason; allowed tables parse.
    let unknown = parse("[capabilities.worker]\nteleport = true").unwrap_err();
    assert!(
        unknown
            .to_string()
            .contains("teleport is not a known capability")
    );
    let ceiling = parse("[capabilities.reviewer]\ntodo = true").unwrap_err();
    assert!(
        ceiling
            .to_string()
            .contains("not available to the reviewer role")
    );
    parse(
        "[capabilities]\npreset = \"full\"\n[capabilities.planner]\n[capabilities.plan_reviewer]",
    )
    .unwrap();
    let mut config = AgentCapabilities::default();
    assert!(config.set(AgentRole::Planner, "todo", true).is_err());
    assert_eq!(config, AgentCapabilities::default());
}

#[test]
fn snapshot_records_values_sources_and_a_stable_digest() {
    // Given the default config and a worker override.
    let standard = AgentCapabilities::default();
    let mut overridden = standard.clone();
    overridden.set(AgentRole::Worker, "todo", false).unwrap();
    // When each resolves the worker snapshot.
    let a = standard.resolve(AgentRole::Worker, None);
    let b = overridden.resolve(AgentRole::Worker, None);
    // Then values, sources and the digest follow the settings deterministically.
    assert_eq!(a, standard.resolve(AgentRole::Worker, None));
    assert_eq!(a.preset, SnapshotPreset::Standard);
    assert_eq!(a.features["todo"].source, CapabilitySource::Preset);
    assert!(a.enabled("todo"));
    assert_eq!(b.features["todo"].source, CapabilitySource::RoleOverride);
    assert!(!b.enabled("todo"));
    assert_ne!(a.digest, b.digest);
    assert!(
        standard
            .resolve(AgentRole::Reviewer, None)
            .enabled("web.enabled")
    );
    assert!(
        !standard
            .resolve(AgentRole::Reviewer, None)
            .features
            .contains_key("todo")
    );
    // And the round trip through TOML preserves the overrides.
    let text = toml::to_string(&overridden).unwrap();
    assert_eq!(
        toml::from_str::<AgentCapabilities>(&text).unwrap(),
        overridden
    );
}

#[test]
fn an_older_host_reports_unavailable_defaults_and_names_explicit_requests() {
    // Given an installed host that implements no catalog key.
    let older = BTreeSet::new();
    let mut explicit = AgentCapabilities::default();
    explicit.set(AgentRole::Pm, "todo", true).unwrap();
    // When resolving a preset default and an explicit request.
    let default = AgentCapabilities::default().resolve(AgentRole::Worker, Some(&older));
    let requested = explicit.resolve(AgentRole::Pm, Some(&older));
    // Then both are omitted and reported; only the explicit request is named for refusal.
    assert!(!default.enabled("todo"));
    assert!(default.features["todo"].unavailable.is_some());
    assert_eq!(default.explicit_unavailable(), None);
    assert!(!requested.enabled("todo"));
    assert_eq!(
        requested.explicit_unavailable().map(|(key, _)| key),
        Some("todo")
    );
    let current: BTreeSet<String> = ["todo".to_owned()].into();
    assert!(
        explicit
            .resolve(AgentRole::Pm, Some(&current))
            .enabled("todo")
    );
}

#[test]
fn legacy_and_changed_settings_produce_an_explicit_transition() {
    // Given a legacy generation with todo and a current strict resolution.
    let legacy = CapabilitySnapshot::legacy(AgentRole::Worker, true);
    let mut strict = AgentCapabilities::default();
    strict.set_preset(Preset::Strict);
    let current = strict.resolve(AgentRole::Worker, None);
    // When the next generation compares itself with the old one.
    let transition = current.transition_from(3, &legacy).unwrap();
    // Then the legacy label and the concrete change are recorded, and identity is silent.
    assert_eq!(legacy.preset, SnapshotPreset::LegacyStrict);
    assert!(legacy.enabled("todo"));
    assert_eq!(transition.from_generation, 3);
    assert_eq!(transition.from_preset, SnapshotPreset::LegacyStrict);
    assert_eq!(transition.from_digest, legacy.digest);
    assert_eq!(
        transition.changes,
        vec![crate::agent::CapabilityChange {
            key: "todo".into(),
            from: Some(true),
            to: Some(false),
            settings: false,
        }]
    );
    assert!(current.transition_from(4, &current).is_none());
}

#[test]
fn web_presets_overrides_and_role_ceilings_share_the_catalog() {
    // Given every preset and role.
    for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
        let mut config = AgentCapabilities::default();
        config.set_preset(preset);
        for role in [
            AgentRole::Worker,
            AgentRole::Reviewer,
            AgentRole::Planner,
            AgentRole::PlanReviewer,
        ] {
            let snapshot = config.resolve(role, None);
            assert_eq!(snapshot.enabled("web.enabled"), preset != Preset::Strict);
            assert_eq!(
                snapshot.web,
                (preset != Preset::Strict).then(WebSettings::default)
            );
            // When web overrides the preset through the existing setter.
            config
                .set(role, "web.enabled", preset == Preset::Strict)
                .unwrap();
            // Then the override is honored without changing the role ceiling.
            assert_eq!(
                config.resolve(role, None).enabled("web.enabled"),
                preset == Preset::Strict
            );
        }
        assert!(config.set(AgentRole::Pm, "web.enabled", true).is_err());
        assert!(config.resolve(AgentRole::Pm, None).web.is_none());
    }
    assert!(toml::from_str::<AgentCapabilities>("[pm.web]\nhosts = ['*']").is_err());
}

#[test]
fn compound_web_config_round_trips_with_todo_overrides_and_endpoint() {
    // Given a strict preset with independent todo and compound web configuration.
    let text = "preset = 'strict'\n[worker]\ntodo = true\n[worker.web]\n\
                enabled = true\nhosts = ['docs.example.org']\n\
                searxng_url = 'http://localhost:8080/search'";
    // When parsed and serialized through the public configuration.
    let config: AgentCapabilities = toml::from_str(text).unwrap();
    let encoded = toml::to_string(&config).unwrap();
    // Then both forms preserve overrides, policy defaults, and the localhost endpoint.
    assert_eq!(
        toml::from_str::<AgentCapabilities>(&encoded).unwrap(),
        config
    );
    let snapshot = config.resolve(AgentRole::Worker, None);
    assert!(snapshot.enabled("todo") && snapshot.enabled("web.enabled"));
    let web = snapshot.web.unwrap();
    assert_eq!(web.hosts, ["docs.example.org"]);
    assert_eq!(web.max_bytes, 1_048_576);
    assert_eq!(web.timeout_ms, 15_000);
    assert_eq!(web.max_redirects, 5);
    assert_eq!(
        web.searxng_url.as_deref(),
        Some("http://localhost:8080/search")
    );
}

#[test]
fn web_config_rejects_invalid_limits_and_endpoints() {
    // Given invalid policy values at the parsing boundary.
    for value in [
        "max_bytes = 0",
        "max_bytes = 9007199254740992",
        "timeout_ms = 0",
        "timeout_ms = 2147483648",
        "max_redirects = -1",
        "max_redirects = 11",
        "searxng_url = 'ftp://localhost/search'",
        "searxng_url = 'https://user:pass@example.org/search'",
        "searxng_url = 'https://example.org/search#fragment'",
        "searxng_url = 'http://localhost:99999'",
        "unknown = true",
    ] {
        // When a compound configuration is parsed, then invalid settings are refused.
        assert!(
            toml::from_str::<AgentCapabilities>(&format!("[worker.web]\n{value}")).is_err(),
            "{value}"
        );
    }
    for url in [
        "http://localhost:8080",
        "https://search.example.org/search",
        "http://[::1]:8080/search",
    ] {
        assert!(
            toml::from_str::<AgentCapabilities>(&format!("[worker.web]\nsearxng_url = '{url}'"))
                .is_ok(),
            "{url}"
        );
    }
}

#[test]
fn host_and_endpoint_settings_each_change_the_snapshot_digest() {
    // Given a standard worker policy and independent host and endpoint changes.
    let original = AgentCapabilities::default().resolve(AgentRole::Worker, None);
    for setting in [
        "hosts = ['docs.example.org']",
        "searxng_url = 'http://localhost:8080'",
    ] {
        let config: AgentCapabilities =
            toml::from_str(&format!("[worker.web]\n{setting}")).unwrap();
        // When resolving settings, then the deterministic digest and concrete diff change.
        let snapshot = config.resolve(AgentRole::Worker, None);
        assert_eq!(snapshot, config.resolve(AgentRole::Worker, None));
        assert_ne!(snapshot.digest, original.digest);
        let transition = snapshot.transition_from(1, &original).unwrap();
        assert!(transition.changes.is_empty());
        assert!(transition.web_change.is_some());
    }
}

#[test]
fn bool_map_web_override_preserves_public_get_and_set_behavior() {
    // Given the existing boolean-map form with a dotted catalog key.
    let mut role: RoleCapabilities =
        toml::from_str("\"web.enabled\" = false\ntodo = true").unwrap();
    assert_eq!(role.get("web.enabled"), Some(false));
    // When the public setter replaces that override.
    role.set("web.enabled", true);
    // Then both getters and the serialized compound form retain the effective values.
    assert_eq!(role.get("web.enabled"), Some(true));
    assert_eq!(role.get("todo"), Some(true));
    let encoded = toml::to_string(&role).unwrap();
    let decoded: RoleCapabilities = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded, role);
}

#[test]
fn web_policy_accepts_transport_boundaries_and_independent_search_endpoint() {
    // Given the exact transport bounds and an empty fetch allowlist.
    let text = "[worker.web]\nhosts = []\nmax_bytes = 9007199254740991\n\
                timeout_ms = 2147483647\nmax_redirects = 10\n\
                searxng_url = 'http://localhost:8080'";
    // When policy resolves, then endpoint configuration does not broaden fetch hosts.
    let config: AgentCapabilities = toml::from_str(text).unwrap();
    let settings = config.resolve(AgentRole::Worker, None).web.unwrap();
    assert!(settings.hosts.is_empty());
    assert_eq!(settings.max_bytes, 9_007_199_254_740_991);
    assert_eq!(settings.timeout_ms, 2_147_483_647);
    assert_eq!(settings.max_redirects, 10);
    assert_eq!(
        settings.searxng_url.as_deref(),
        Some("http://localhost:8080")
    );
}

const ALL: [AgentRole; 5] = ROLES;

#[test]
fn guidance_and_session_keys_follow_the_role_matrix() {
    // Given each preset.
    for (index, preset) in [Preset::Strict, Preset::Standard, Preset::Full]
        .into_iter()
        .enumerate()
    {
        let mut config = AgentCapabilities::default();
        config.set_preset(preset);
        for role in ALL {
            // When the role resolves its snapshot.
            let snapshot = config.resolve(role, None);
            // Then optional text is off only in strict, compaction keeps upstream's default
            // on, lessons stay on, and PM has neither lessons nor export.
            let optional = index > 0;
            for key in ["context_files", "skills", "prompt_templates"] {
                assert_eq!(snapshot.enabled(key), optional, "{role:?} {key}");
            }
            assert!(snapshot.enabled("compaction"));
            let pm = role == AgentRole::Pm;
            assert_eq!(snapshot.features.contains_key("lessons"), !pm);
            assert_eq!(snapshot.enabled("lessons"), !pm);
            assert_eq!(snapshot.enabled("session_export"), optional && !pm);
            assert_eq!(snapshot.features.contains_key("session_export"), !pm);
        }
    }
    // And a ceiling-disallowed override is a configuration error.
    let mut config = AgentCapabilities::default();
    assert!(config.set(AgentRole::Pm, "lessons", true).is_err());
    assert!(config.set(AgentRole::Pm, "session_export", true).is_err());
}

fn user_dirs(snapshot: &CapabilitySnapshot, key: &str) -> serde_json::Value {
    snapshot.features[key].settings["user_dirs"].clone()
}

#[test]
fn home_skill_directories_default_for_every_role_and_can_be_added_or_disabled() {
    // Given a worker that disables home skills, a PM that adds a directory and a planner
    // that turns skills off.
    let config: crate::config::AgentCfg = toml::from_str(
        "[capabilities.worker.skills]\nuser_dirs = []\n\
         [capabilities.pm.skills]\nuser_dirs = [\"~/.agents/skills\", \"/opt/team-skills\"]\n\
         [capabilities.planner]\nskills = false\n\
         [capabilities.reviewer.prompt_templates]\nenabled = false\nuser_dirs = [\"/opt/prompts\"]",
    )
    .unwrap();
    let caps = &config.capabilities;
    // Then the default reaches every other role, and each override applies only to its role.
    let standard = AgentCapabilities::default();
    for role in ALL {
        assert_eq!(
            user_dirs(&standard.resolve(role, None), "skills"),
            serde_json::json!(["~/.agents/skills", "~/.corral/skills/pi-native"])
        );
        assert_eq!(
            user_dirs(&standard.resolve(role, None), "prompt_templates"),
            serde_json::json!(["~/.agents/prompts"])
        );
    }
    let worker = caps.resolve(AgentRole::Worker, None);
    assert!(worker.enabled("skills"));
    assert_eq!(worker.features["skills"].source, CapabilitySource::Preset);
    assert_eq!(user_dirs(&worker, "skills"), serde_json::json!([]));
    let pm = caps.resolve(AgentRole::Pm, None);
    assert_eq!(
        user_dirs(&pm, "skills"),
        serde_json::json!(["~/.agents/skills", "/opt/team-skills"])
    );
    let planner = caps.resolve(AgentRole::Planner, None);
    assert!(!planner.enabled("skills"));
    assert!(planner.features["skills"].settings.is_empty());
    let reviewer = caps.resolve(AgentRole::Reviewer, None);
    assert!(!reviewer.enabled("prompt_templates"));
    assert_eq!(
        reviewer.features["prompt_templates"].source,
        CapabilitySource::RoleOverride
    );
    assert_eq!(
        user_dirs(&caps.resolve(AgentRole::PlanReviewer, None), "skills"),
        serde_json::json!(["~/.agents/skills", "~/.corral/skills/pi-native"])
    );
    // And the settings round-trip through TOML and change the digest.
    let text = toml::to_string(caps).unwrap();
    assert_eq!(&toml::from_str::<AgentCapabilities>(&text).unwrap(), caps);
    assert_ne!(
        worker.digest,
        standard.resolve(AgentRole::Worker, None).digest
    );
}

#[test]
fn compaction_settings_are_validated_recorded_and_reported_on_change() {
    // Given configured compaction and branch-summary settings.
    let config: crate::config::AgentCfg = toml::from_str(
        "[capabilities.worker.compaction]\nreserve_tokens = 8192\nkeep_recent_tokens = 12000\n\
         branch_summary = { reserve_tokens = 4096, skip_prompt = true }",
    )
    .unwrap();
    let configured = config.capabilities.resolve(AgentRole::Worker, None);
    let default = AgentCapabilities::default().resolve(AgentRole::Worker, None);
    // Then only configured values are recorded; omitted ones stay upstream defaults.
    assert!(default.features["compaction"].settings.is_empty());
    assert_eq!(
        serde_json::to_value(&configured.features["compaction"].settings).unwrap(),
        serde_json::json!({"reserve_tokens": 8192, "keep_recent_tokens": 12000,
            "branch_summary": {"reserve_tokens": 4096, "skip_prompt": true}})
    );
    // And a resume after the change records it as a settings change of an enabled key.
    let transition = configured.transition_from(1, &default).unwrap();
    assert_eq!(
        transition.changes,
        vec![crate::agent::CapabilityChange {
            key: "compaction".into(),
            from: Some(true),
            to: Some(true),
            settings: true,
        }]
    );
    assert!(transition.summary().contains("settings changed"));
    // And malformed settings are configuration errors that name the table.
    let parse = |text: &str| {
        toml::from_str::<crate::config::AgentCfg>(text)
            .map(|_| ())
            .unwrap_err()
            .to_string()
    };
    for (text, reason) in [
        (
            "[capabilities.worker.compaction]\nreserve_tokens = -1",
            "non-negative integer",
        ),
        (
            "[capabilities.worker.compaction]\nwindow = 1",
            "has no setting window",
        ),
        (
            "[capabilities.worker.compaction]\nbranch_summary = { depth = 1 }",
            "branch_summary.depth",
        ),
        (
            "[capabilities.worker.skills]\nuser_dirs = [\"relative/skills\"]",
            "absolute or ~/ directory",
        ),
        (
            "[capabilities.worker.todo]\nenabled = true\nlimit = 3",
            "takes only `enabled`",
        ),
        (
            "[capabilities.worker.skills]\nenabled = \"yes\"",
            "enabled must be a boolean",
        ),
    ] {
        let error = parse(text);
        assert!(error.contains(reason), "{text}: {error}");
        assert!(error.contains("[agent.capabilities.worker."), "{error}");
    }
}

#[test]
fn project_text_trust_is_one_recorded_decision_per_project() {
    // Given the default and a configuration that trusts only one project.
    let default = AgentCapabilities::default();
    let config: crate::config::AgentCfg =
        toml::from_str("[capabilities]\ntrusted_projects = [\"corral\"]").unwrap();
    // Then every registered project is trusted by default, the list narrows it, an empty
    // list trusts none, and doctor's project-free view records no decision.
    let trust = |caps: &AgentCapabilities, project| {
        caps.resolve_for(AgentRole::Worker, Some(project), None)
            .project_text
    };
    assert_eq!(trust(&default, "anything"), Some(true));
    assert_eq!(trust(&config.capabilities, "corral"), Some(true));
    assert_eq!(trust(&config.capabilities, "mitiplay"), Some(false));
    let mut none = AgentCapabilities::default();
    none.set_trusted_projects(vec![]);
    assert_eq!(trust(&none, "corral"), Some(false));
    assert_eq!(default.resolve(AgentRole::Worker, None).project_text, None);
    // And the decision is part of the digest.
    assert_ne!(
        default
            .resolve_for(AgentRole::Worker, Some("corral"), None)
            .digest,
        none.resolve_for(AgentRole::Worker, Some("corral"), None)
            .digest
    );
    assert!(
        !toml::to_string(&default)
            .unwrap()
            .contains("trusted_projects")
    );
}

#[test]
fn rust_side_lessons_never_depend_on_the_installed_host() {
    // Given an installed host that implements no catalog key.
    let older = BTreeSet::new();
    let snapshot = AgentCapabilities::default().resolve(AgentRole::Worker, Some(&older));
    // Then host features are reported unavailable while prompt lessons stay on.
    assert!(snapshot.enabled("lessons"));
    assert!(snapshot.features["lessons"].unavailable.is_none());
    assert!(!snapshot.enabled("skills"));
    assert!(snapshot.features["skills"].unavailable.is_some());
    // And a legacy record takes the strict values: compaction and lessons on, text off.
    let legacy = CapabilitySnapshot::legacy(AgentRole::Reviewer, false);
    assert!(legacy.enabled("compaction") && legacy.enabled("lessons"));
    assert!(!legacy.enabled("skills") && !legacy.enabled("session_export"));
}

#[test]
fn hooks_and_bash_rules_follow_full_preset_ceilings_and_record_effective_settings() {
    // Given full-preset tables for roles that may and may not use each control.
    let text = "preset = 'full'\n[worker.hooks]\ntimeout_ms = 500\n\
                [[worker.hooks.pre_tool]]\nname = 'lint'\nargv = ['lint', '--fix']\ntools = ['edit']\n\
                [reviewer.bash_rules]\ndeny = ['git push*']\ndefault = 'ask'";
    let config: AgentCapabilities = toml::from_str(text).unwrap();
    // When each role resolves.
    let worker = config.resolve(AgentRole::Worker, None);
    let reviewer = config.resolve(AgentRole::Reviewer, None);
    // Then the effective settings are complete and the worker's typed hooks are readable.
    let hooks = crate::agent::HookSettings::of(&worker).unwrap();
    assert_eq!(hooks.timeout_ms, 500);
    assert!(hooks.wraps("edit") && !hooks.wraps("read") && !hooks.wraps("corral_edit"));
    assert_eq!(
        serde_json::to_value(&worker.features["bash_rules"].settings).unwrap(),
        serde_json::json!({"allow": [], "ask": [], "deny": [], "default": "allow"})
    );
    assert_eq!(
        reviewer.features["bash_rules"].settings["default"],
        serde_json::json!("ask")
    );
    assert!(!reviewer.features.contains_key("hooks"));
    assert!(
        !config
            .resolve(AgentRole::Pm, None)
            .features
            .contains_key("bash_rules")
    );
    // And standard leaves both off until a role override enables one.
    let mut standard = AgentCapabilities::default();
    let before = standard.resolve(AgentRole::Worker, None);
    assert!(!before.enabled("hooks") && !before.enabled("bash_rules"));
    assert!(crate::agent::HookSettings::of(&before).is_none());
    standard.set(AgentRole::Worker, "hooks", true).unwrap();
    let after = standard.resolve(AgentRole::Worker, None);
    assert_eq!(
        crate::agent::HookSettings::of(&after),
        Some(crate::agent::HookSettings::default())
    );
    let transition = after.transition_from(1, &before).unwrap();
    assert!(
        transition
            .changes
            .iter()
            .any(|c| c.key == "hooks" && c.to == Some(true))
    );
    // And role ceilings and invalid settings are configuration errors.
    for invalid in [
        "[reviewer.hooks]\nenabled = true",
        "[pm]\nbash_rules = true",
        "[worker.hooks]\n[[worker.hooks.pre_tool]]\nname = 'x'\nargv = []",
        "[worker.hooks]\n[[worker.hooks.post_tool]]\nname = 'x'\nargv = ['x']\ntools = ['corral_done']",
        "[worker.bash_rules]\ndefault = 'maybe'",
        "[worker.bash_rules]\nunknown = 1",
    ] {
        assert!(
            toml::from_str::<AgentCapabilities>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn background_shell_is_worker_only_with_recorded_bounds() {
    // Given each preset, then only the worker resolves background_shell, off in strict.
    for (index, preset) in [Preset::Strict, Preset::Standard, Preset::Full]
        .into_iter()
        .enumerate()
    {
        let mut config = AgentCapabilities::default();
        config.set_preset(preset);
        for role in ALL {
            let snapshot = config.resolve(role, None);
            let worker = role == AgentRole::Worker;
            assert_eq!(snapshot.features.contains_key("background_shell"), worker);
            assert_eq!(snapshot.enabled("background_shell"), worker && index > 0);
        }
    }
    // When a worker narrows the bounds, they are recorded effective and change the digest.
    let config: crate::config::AgentCfg = toml::from_str(
        "[capabilities.worker.background_shell]\nmax_jobs = 2\nmax_runtime_secs = 600",
    )
    .unwrap();
    let narrowed = config.capabilities.resolve(AgentRole::Worker, None);
    assert_eq!(
        serde_json::Value::Object(
            narrowed.features["background_shell"]
                .settings
                .clone()
                .into_iter()
                .collect()
        ),
        serde_json::json!({"max_jobs": 2, "max_output_bytes": 8388608, "max_runtime_secs": 600})
    );
    let standard = AgentCapabilities::default().resolve(AgentRole::Worker, None);
    assert_ne!(narrowed.digest, standard.digest);
    // And other roles, unknown settings and out-of-range bounds are configuration errors.
    for text in [
        "[capabilities.reviewer]\nbackground_shell = true",
        "[capabilities.pm.background_shell]\nmax_jobs = 1",
        "[capabilities.worker.background_shell]\nmax_jobs = 0",
        "[capabilities.worker.background_shell]\nmax_output_bytes = 10",
        "[capabilities.worker.background_shell]\nshell = \"zsh\"",
    ] {
        assert!(
            toml::from_str::<crate::config::AgentCfg>(text).is_err(),
            "{text}"
        );
    }
}

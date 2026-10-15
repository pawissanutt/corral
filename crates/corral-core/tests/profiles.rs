use corral_core::{Config, FlagMap, Harness, Profile, ProfileChoice};
use tempfile::tempdir;

/// The workspace `.corral/corral.toml` written before profiles existed (agents only).
const LEGACY_TOML: &str = r#"[limits]
running = 3

[limits.per_project]

[projects.corral]
path = "corral"

[projects.mitiplay]
path = "mitiplay"

[agents.claude]
command = """claude "$(cat '{prompt_file}')""""

[agents.generic]
command = "cat '{prompt_file}'"

[agents.omo]
command = """omo -n '{name}' "$(cat '{prompt_file}')""""

[agents.omp]
command = """omp "$(cat '{prompt_file}')""""

[artifacts]
globs = [
    ".omo/evidence/**",
    ".omo/plans/*.md",
    "docs/**/records/*",
    "**/*report*.html",
    "**/*report*.md",
]
max_bytes = 20000000

[herdr]

[pm]
agent = "omo"
"#;

fn choice(profile: Option<&str>, model: Option<&str>, effort: Option<&str>) -> ProfileChoice {
    ProfileChoice {
        profile: profile.map(str::to_string),
        model: model.map(str::to_string),
        effort: effort.map(str::to_string),
    }
}

fn harness(command: &str) -> Harness {
    Harness {
        command: command.into(),
        flags: None,
        model_flag: None,
        effort_flag: None,
        efforts: Vec::new(),
        prompt: None,
    }
}

fn profile(harness: &str, model: Option<&str>, effort: Option<&str>) -> Profile {
    Profile {
        harness: harness.into(),
        model: model.map(str::to_string),
        vendor: None,
        escalate_to: None,
        effort: effort.map(str::to_string),
        args: Vec::new(),
    }
}

#[test]
fn legacy_agents_config_parses_round_trips_and_resolves_unchanged() {
    let d = tempdir().unwrap();
    let f = d.path().join("corral.toml");
    std::fs::write(&f, LEGACY_TOML).unwrap();
    let c = Config::load(&f).unwrap();
    assert_eq!(c.agents.len(), 4);
    assert!(c.harnesses.is_empty() && c.profiles.is_empty());
    assert_eq!(c.defaults.profile, None);
    assert_eq!(c.pm.agent, "omo");

    let r = c
        .resolve_profile(&ProfileChoice::default(), None, None)
        .unwrap();
    assert_eq!(r.profile, "omo");
    assert_eq!(r.harness, "omo");
    assert_eq!(r.command, c.agents["omo"].command);
    assert!(r.model_args.is_empty() && r.model.is_none() && r.effort.is_none());
    let r = c
        .resolve_profile(&choice(Some("claude"), None, None), None, None)
        .unwrap();
    assert_eq!(r.command, "claude \"$(cat '{prompt_file}')\"");

    let err = c
        .resolve_profile(&choice(None, Some("opus"), None), None, None)
        .unwrap_err();
    assert!(format!("{err:#}").contains("{model_args}"), "{err:#}");

    let out = d.path().join("out.toml");
    std::fs::write(&out, c.to_toml()).unwrap();
    assert_eq!(Config::load(&out).unwrap(), c);
    assert!(!c.to_toml().contains("[profiles"), "{}", c.to_toml());
}

#[test]
fn legacy_default_agent_key_reads_as_default_profile() {
    let c: Config = toml::from_str(
        "[projects.app]\npath = \"app\"\ndefault_agent = \"claude\"\n\n[agents.claude]\ncommand = \"claude\"\n",
    )
    .unwrap();
    assert_eq!(c.projects["app"].default_profile.as_deref(), Some("claude"));
    assert_eq!(c.default_profile(None, Some("app")), "claude");
}

#[test]
fn builtin_flag_maps_render_per_harness() {
    let omo = FlagMap::builtin("omo").unwrap();
    assert_eq!(
        omo.render("omo", Some("opus"), Some("XHigh")).unwrap(),
        ["--model", "opus", "--thinking", "xhigh"]
    );
    assert_eq!(
        omo.render("omo", None, Some("off")).unwrap(),
        ["--thinking", "off"]
    );
    assert!(omo.render("omo", None, Some("auto")).is_err());

    let omp = FlagMap::builtin("omp").unwrap();
    assert_eq!(
        omp.render("omp", Some("gpt-5.2"), Some("auto")).unwrap(),
        ["--model", "gpt-5.2", "--thinking", "auto"]
    );

    let claude = FlagMap::builtin("claude").unwrap();
    assert_eq!(
        claude
            .render("claude", Some("sonnet"), Some("max"))
            .unwrap(),
        ["--model", "sonnet", "--effort", "max"]
    );
    assert!(claude.render("claude", None, None).unwrap().is_empty());

    let generic = FlagMap::builtin("generic").unwrap();
    assert!(generic.render("generic", None, None).unwrap().is_empty());
    let err = generic.render("generic", Some("x"), None).unwrap_err();
    assert!(format!("{err:#}").contains("no model flag"), "{err:#}");
    assert!(FlagMap::builtin("codex").is_none());
}

#[test]
fn unknown_effort_lists_valid_levels() {
    let claude = FlagMap::builtin("claude").unwrap();
    let err = claude.render("claude", None, Some("off")).unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "unknown effort 'off' for harness 'claude' (valid: low, medium, high, xhigh, max)"
    );

    let mut c = Config::default_for(tempdir().unwrap().path());
    let err = c
        .resolve_profile(&choice(Some("omo"), None, Some("turbo")), None, None)
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("'turbo'") && msg.contains("off, minimal, low, medium, high, xhigh, max"),
        "{msg}"
    );

    let mut custom = harness("x {model_args}");
    custom.flags = Some("claude".into());
    custom.effort_flag = Some("--reasoning".into());
    custom.efforts = vec!["Fast".into(), "deep".into()];
    c.harnesses.insert("x".into(), custom);
    c.profiles
        .insert("x".into(), profile("x", None, Some("deep")));
    let r = c
        .resolve_profile(&choice(Some("x"), Some("m"), None), None, None)
        .unwrap();
    assert_eq!(r.model_args, ["--model", "m", "--reasoning", "deep"]);
    let err = c
        .resolve_profile(&choice(Some("x"), None, Some("max")), None, None)
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("(valid: fast, deep)"),
        "{err:#}"
    );
}

#[test]
fn resolution_order_and_overrides() {
    let d = tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("app/.git")).unwrap();
    let mut c = Config::default_for(d.path());
    c.profiles.insert(
        "deep".into(),
        Profile {
            args: vec!["--plan".into()],
            ..profile("claude", Some("opus"), Some("high"))
        },
    );
    c.profiles
        .insert("fast".into(), profile("omp", Some("haiku"), Some("low")));
    c.profiles.insert("proj".into(), profile("omo", None, None));
    c.profiles
        .insert("glob".into(), profile("generic", None, None));

    let name = |c: &Config, task: Option<&str>| c.default_profile(task, Some("app"));
    assert_eq!(name(&c, None), "omo");
    c.defaults.profile = Some("glob".into());
    assert_eq!(name(&c, None), "glob");
    c.projects.get_mut("app").unwrap().default_profile = Some("proj".into());
    assert_eq!(name(&c, None), "proj");
    assert_eq!(c.default_profile(None, Some("other")), "glob");
    assert_eq!(name(&c, Some("fast")), "fast");

    let r = c
        .resolve_profile(&choice(Some("deep"), None, None), Some("fast"), Some("app"))
        .unwrap();
    assert_eq!((r.profile.as_str(), r.harness.as_str()), ("deep", "claude"));
    assert_eq!(
        r.model_args,
        ["--model", "opus", "--effort", "high", "--plan"]
    );

    let r = c
        .resolve_profile(
            &choice(None, Some("sonnet"), Some("MAX")),
            Some("deep"),
            Some("app"),
        )
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("sonnet"));
    assert_eq!(r.effort.as_deref(), Some("max"));

    let r = c
        .resolve_profile(&ProfileChoice::default(), None, Some("app"))
        .unwrap();
    assert_eq!((r.profile.as_str(), r.harness.as_str()), ("proj", "omo"));
    assert!(r.model_args.is_empty());

    let err = c
        .resolve_profile(&choice(Some("nope"), None, None), None, None)
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unknown profile 'nope'") && msg.contains("deep"),
        "{msg}"
    );
}

#[test]
fn profiles_round_trip_through_toml() {
    let d = tempdir().unwrap();
    let mut c = Config::default_for(d.path());
    c.defaults.profile = Some("deep".into());
    c.profiles
        .insert("deep".into(), profile("claude", Some("opus"), Some("max")));
    let f = d.path().join("c.toml");
    std::fs::write(&f, c.to_toml()).unwrap();
    assert_eq!(Config::load(&f).unwrap(), c);
}

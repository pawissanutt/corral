use super::*;
use crate::model::{Priority, Status};

fn task() -> Task {
    Task {
        id: "T-042".into(),
        title: "Frontend layout".into(),
        body: String::new(),
        status: Status::Backlog,
        project: Some("app".into()),
        priority: Priority::P2,
        kind: Some("ui".into()),
        size: Some("l".into()),
        profile: None,
        base: None,
        parent: None,
        depends_on: Vec::new(),
        blocked: None,
        round: 0,
        created: jiff::Timestamp::UNIX_EPOCH,
        updated: jiff::Timestamp::UNIX_EPOCH,
    }
}

#[test]
fn parses_complete_contract_and_preserves_builtins() {
    let root = crate::tests::tempdir();
    std::fs::create_dir(root.path().join("app")).unwrap();
    let path = root.path().join("corral.toml");
    std::fs::write(
        &path,
        r#"
[limits]
running = 7
[defaults]
profile = "sol"
base = "trunk"
max_rounds = 3
auto_dispatch = true
auto_land = true
branch_prefix = "work/"
[runner]
mode = "herdr"
herdr_session = "isolated"
[sync]
quiet_after_secs = 60
[artifacts]
globs = ["reports/*.html"]
max_bytes = 1234
[harnesses.omp]
command = "custom {model_args} {prompt_ref}"
model_flag = "--model"
effort_flag = "--effort"
[harnesses.other]
command = "other {prompt_file}"
[profiles.sol]
harness = "omp"
model = "gpt-6.2-sol"
effort = "medium"
args = ["--verbose"]
[projects.app]
path = "app"
base = "develop"
gate = ["just check", "just test"]
reviewer = "sol"
profile = "sol"
running = 2
auto_dispatch = false
auto_land = false
max_rounds = 1
branch_prefix = "app/"
[[routing]]
kind = ["ui"]
size = ["l", "xl"]
title_regex = "(?i)frontend"
project = "app"
profile = "sol"
"#,
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    assert_eq!(config.limits.running, 7);
    assert_eq!(config.runner.herdr_session.as_deref(), Some("isolated"));
    assert_eq!(config.sync.quiet_after_secs, 60);
    assert_eq!(config.artifacts.max_bytes, 1234);
    assert_eq!(config.harnesses.len(), 6);
    assert!(config.harnesses["omp"].command.starts_with("custom "));
    assert!(config.profiles.contains_key("generic"));
    assert_eq!(config.resolve_profile(&task(), None).unwrap(), "sol");
    let project = config.effective_project("app").unwrap();
    assert_eq!(project.path, Path::new("app"));
    assert_eq!(project.base.as_deref(), Some("develop"));
    assert_eq!(project.gate, ["just check", "just test"]);
    assert_eq!(project.reviewer.as_deref(), Some("sol"));
    assert_eq!(project.running, Some(2));
    assert!(!project.auto_dispatch);
    assert!(!project.auto_land);
    assert_eq!(project.max_rounds, 1);
    assert_eq!(project.branch_prefix, "app/");
    assert!(config.validate(root.path()).is_empty());
}

#[test]
fn defaults_and_project_fallback_preserve_explicit_false_and_zero_rounds() {
    let mut config: Config = toml::from_str(
        r#"
[projects.app]
path = "."
gate = "just check"
"#,
    )
    .unwrap();
    assert_eq!(config.runner.mode, "herdr");
    assert_eq!(config.limits.running, 4);
    assert_eq!(config.defaults.base, None);
    assert_eq!(config.defaults.branch_prefix, "corral/");
    assert_eq!(config.defaults.max_rounds, 2);
    assert_eq!(config.defaults.profile.as_deref(), Some("omp-sol"));
    assert_eq!(config.harnesses.len(), 5);
    assert_eq!(config.profiles.len(), 6);
    assert_eq!(config.effective_project("app").unwrap().base, None);
    config.defaults.base = Some("trunk".into());
    config.defaults.profile = Some("omp".into());
    config.defaults.auto_dispatch = true;
    config.defaults.auto_land = true;
    let inherited = config.effective_project("app").unwrap();
    assert_eq!(inherited.profile.as_deref(), Some("omp"));
    assert_eq!(inherited.base.as_deref(), Some("trunk"));
    assert!(inherited.auto_dispatch && inherited.auto_land);
    assert_eq!(inherited.gate, ["just check"]);
    assert!(inherited.running.is_none());
    let project = config.projects.get_mut("app").unwrap();
    project.auto_dispatch = Some(false);
    project.auto_land = Some(false);
    project.max_rounds = Some(0);
    let overridden = config.effective_project("app").unwrap();
    assert!(!overridden.auto_dispatch && !overridden.auto_land);
    assert_eq!(overridden.max_rounds, 0);
    assert!(config.effective_project("missing").is_err());
    let round_trip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(round_trip.projects["app"].gate, ["just check"]);
}

#[test]
fn profile_resolution_obeys_all_five_levels_and_first_matching_rule() {
    let mut config: Config = toml::from_str(
        r#"
[defaults]
profile = "generic"
[projects.app]
path = "."
profile = "claude"
[[routing]]
kind = ["ui"]
size = ["l", "xl"]
title_regex = "(?i)frontend"
project = "app"
profile = "omo"
[[routing]]
profile = "omp"
"#,
    )
    .unwrap();
    let mut task = task();
    task.profile = Some("generic".into());
    assert_eq!(
        config.resolve_profile(&task, Some("claude")).unwrap(),
        "claude"
    );
    assert_eq!(config.resolve_profile(&task, None).unwrap(), "generic");
    assert!(config.resolve_profile(&task, Some("missing")).is_err());
    task.profile = None;
    assert_eq!(config.resolve_profile(&task, None).unwrap(), "omo");
    for field in ["kind", "size", "title", "project"] {
        let mut different = task.clone();
        match field {
            "kind" => different.kind = None,
            "size" => different.size = Some("s".into()),
            "title" => different.title = "backend".into(),
            "project" => different.project = None,
            _ => unreachable!(),
        }
        assert_eq!(
            config.resolve_profile(&different, None).unwrap(),
            "omp",
            "{field}"
        );
    }
    config.routing.clear();
    assert_eq!(config.resolve_profile(&task, None).unwrap(), "claude");
    config.projects.get_mut("app").unwrap().profile = None;
    assert_eq!(config.resolve_profile(&task, None).unwrap(), "generic");
    config.defaults.profile = None;
    let error = config.resolve_profile(&task, None).unwrap_err();
    assert!(error.to_string().contains("no worker profile"));
    task.profile = Some("missing".into());
    assert!(config.resolve_profile(&task, None).is_err());
}

#[test]
fn validation_collects_independent_invalid_references_and_values() {
    let root = crate::tests::tempdir();
    let config: Config = toml::from_str(
        r#"
[limits]
running = 0
[defaults]
profile = "missing-default"
base = ""
[runner]
mode = "unknown"
[artifacts]
globs = ["["]
[harnesses.bad]
command = ""
[profiles.bad]
harness = "missing-harness"
[profiles.no-flags]
harness = "generic"
model = "model"
effort = "effort"
fallback = []
[projects.app]
path = "missing-directory"
running = 0
profile = "missing-worker"
reviewer = "missing-reviewer"
gate = [""]
[[routing]]
profile = "missing-route"
project = "missing-project"
title_regex = "["
"#,
    )
    .unwrap();
    let problems = config.validate(root.path());
    for needle in [
        "limits.running",
        "defaults.profile",
        "defaults.base",
        "runner.mode",
        "artifacts.globs",
        "harnesses.bad.command",
        "profiles.bad.harness",
        "profiles.no-flags.model",
        "profiles.no-flags.effort",
        "profiles.no-flags.fallback",
        "projects.app.path",
        "projects.app.running",
        "projects.app.profile",
        "projects.app.reviewer",
        "projects.app.gate",
        "routing[0].profile",
        "routing[0].project",
        "routing[0].title_regex",
    ] {
        assert!(
            problems
                .iter()
                .any(|problem| problem.key.as_deref() == Some(needle)),
            "missing {needle}: {problems:?}"
        );
    }
    let mut matching_task = task();
    matching_task.project = Some("missing-project".into());
    assert!(
        config
            .resolve_profile(&matching_task, None)
            .unwrap_err()
            .to_string()
            .contains("routing[0]")
    );
}

#[test]
fn load_reports_file_context_and_rejects_unknown_or_mistyped_fields() {
    let root = crate::tests::tempdir();
    let path = root.path().join("corral.toml");
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("corral.toml")
    );
    for text in [
        "[defaults]\nauto_lnad = true",
        "[projects.app]\ngate = 3",
        "[profiles.pi]\nharness = 'pi'\nfallback = ['unqualified']",
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(
            Config::load(&path)
                .unwrap_err()
                .to_string()
                .contains("parsing configuration")
        );
    }
}

#[cfg(unix)]
fn argv(command: &str) -> Vec<String> {
    let result = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = String::from_utf8(result.stdout).unwrap();
    text.strip_suffix('\0')
        .unwrap()
        .split('\0')
        .map(str::to_owned)
        .collect()
}

#[cfg(unix)]
#[test]
fn pi_profile_fallback_renders_ordered_flags_even_without_model_args() {
    let config = Config::check(
        Path::new("."),
        r#"
[profiles.pi]
harness = "pi"
fallback = ["anthropic-api/claude-sonnet-4-6", "openai/gpt-5.4"]
"#,
    )
    .unwrap();
    let config: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    let harness = Harness {
        command: "printf '%s\\0' run".into(),
        ..Harness::default()
    };
    let context = CommandContext {
        prompt_file: Path::new("prompt.md"),
        name: "T-042",
        task: "T-042",
        worktree: Path::new("."),
        corral: Path::new("corral-next"),
    };
    let rendered = render_command(&harness, &config.profiles["pi"], &context).unwrap();
    assert_eq!(
        argv(&rendered),
        [
            "run",
            "--fallback",
            "anthropic-api/claude-sonnet-4-6",
            "--fallback",
            "openai/gpt-5.4",
        ]
    );
}

#[test]
fn pi_api_dispatch_uses_the_provider_qualified_model_without_args() {
    let context = CommandContext {
        prompt_file: Path::new("prompt.md"),
        name: "T-042",
        task: "T-042",
        worktree: Path::new("."),
        corral: Path::new("corral-next"),
    };

    let api = Config::check(
        Path::new("."),
        r#"
[profiles.pi-api]
harness = "pi"
model = "anthropic-api/claude-opus-5-5"
"#,
    )
    .unwrap();
    let harness = Harness {
        command: "printf '%s\\0' run {model_args}".into(),
        model_flag: Some("--model".into()),
        ..Harness::default()
    };
    let rendered = render_command(&harness, &api.profiles["pi-api"], &context).unwrap();
    assert_eq!(
        argv(&rendered),
        ["run", "--model", "anthropic-api/claude-opus-5-5"]
    );
}

#[cfg(unix)]
#[test]
fn rendering_preserves_every_argument_in_unquoted_and_legacy_quoted_templates() {
    let context = CommandContext {
        prompt_file: Path::new("/tmp/prompt ' \" $(printf injected) {task}.md"),
        name: "T-042 Jane's \"name\" $HOME `printf injected` {worktree}",
        task: "T-042; printf injected",
        worktree: Path::new("/tmp/work tree's \"$HOME\""),
        corral: Path::new("/tmp/corral ' $HOME"),
    };
    let profile = Profile {
        harness: "custom".into(),
        model: Some("model ' $(printf injected)".into()),
        effort: Some("high \" $HOME".into()),
        args: vec![
            "".into(),
            "two words".into(),
            "'; printf injected; '".into(),
            "{name}".into(),
        ],
        ..Profile::default()
    };
    for quote in ["", "'", "\""] {
        let placeholders = [
            "prompt_file",
            "prompt_ref",
            "name",
            "task",
            "worktree",
            "corral",
            "model_args",
        ]
        .map(|token| format!("{quote}{{{token}}}{quote}"))
        .join(" ");
        let harness = Harness {
            command: format!("printf '%s\\0' {placeholders}"),
            model_flag: Some("--model".into()),
            effort_flag: Some("--effort".into()),
        };
        let rendered = render_command(&harness, &profile, &context).unwrap();
        assert_eq!(
            argv(&rendered),
            vec![
                context.prompt_file.to_str().unwrap().to_owned(),
                format!(
                    "Read {} and follow its instructions.",
                    context.prompt_file.display()
                ),
                context.name.into(),
                context.task.into(),
                context.worktree.to_str().unwrap().into(),
                context.corral.to_str().unwrap().into(),
                "--model".into(),
                profile.model.clone().unwrap(),
                "--effort".into(),
                profile.effort.clone().unwrap(),
                "".into(),
                "two words".into(),
                "'; printf injected; '".into(),
                "{name}".into(),
            ],
            "quote {quote:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn rendering_builtins_and_embedded_quotes_does_not_double_quote_values() {
    let config = Config::default();
    let context = CommandContext {
        prompt_file: Path::new("prompt ' file.md"),
        name: "T-042 O'Brien",
        task: "T-042",
        worktree: Path::new("."),
        corral: Path::new("corral-next"),
    };
    for name in ["omo", "omp", "claude", "pi", "generic"] {
        let builtin = &config.harnesses[name];
        let rendered = render_command(builtin, &config.profiles[name], &context).unwrap();
        assert!(!rendered.contains("{prompt_"));
        assert!(!rendered.contains("{model_args}"));
        match name {
            "pi" => {
                assert!(rendered.starts_with("corral-pi run "));
                assert!(rendered.contains(" --prompt-file "));
                assert!(rendered.contains(" --corral "));
            }
            "generic" => assert!(rendered.starts_with("cat ")),
            _ => assert!(rendered.starts_with(name)),
        }
    }
    let harness = Harness {
        command: "printf '%s\\0' prefix'{name}'suffix \"prefix{corral}suffix\"".into(),
        ..Harness::default()
    };
    let rendered = render_command(&harness, &Profile::default(), &context).unwrap();
    assert_eq!(
        argv(&rendered),
        vec![
            format!("prefix{}suffix", context.name),
            format!(
                "prefix{}suffix",
                std::path::absolute(context.corral).unwrap().display()
            ),
        ]
    );
}

#[test]
fn rendering_refuses_nul_missing_flags_and_ambiguous_argv_templates() {
    let context = CommandContext {
        prompt_file: Path::new("prompt.md"),
        name: "bad\0name",
        task: "T-042",
        worktree: Path::new("."),
        corral: Path::new("corral-next"),
    };
    let mut harness = Harness {
        command: "tool {name}".into(),
        ..Harness::default()
    };
    assert!(render_command(&harness, &Profile::default(), &context).is_err());
    harness.command = "tool {model_args}".into();
    let profile = Profile {
        model: Some("model".into()),
        ..Profile::default()
    };
    assert!(render_command(&harness, &profile, &context).is_err());
    harness.command = "tool \"prefix{model_args}\"".into();
    assert!(render_command(&harness, &Profile::default(), &context).is_err());
}

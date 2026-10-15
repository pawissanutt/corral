use std::path::Path;

use corral_core::{Config, Harness, Store};

use super::*;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/harness");

fn fixture(name: &str) -> String {
    let path = Path::new(FIXTURES).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn harness(command: &str, flags: Option<&str>) -> Harness {
    Harness {
        command: command.to_string(),
        flags: flags.map(str::to_string),
        model_flag: None,
        effort_flag: None,
        efforts: Vec::new(),
        prompt: None,
    }
}

#[test]
fn parses_captured_versions() {
    assert_eq!(
        parse_version(&fixture("omo-version.txt")).as_deref(),
        Some("5.1.33")
    );
    assert_eq!(
        parse_version(&fixture("omp-version.txt")).as_deref(),
        Some("18.4.0")
    );
    assert_eq!(
        parse_version(&fixture("claude-version.txt")).as_deref(),
        Some("2.1.299")
    );
    assert_eq!(parse_version("tool v1.2.3\n").as_deref(), Some("1.2.3"));
    assert_eq!(
        parse_version("\n  nightly build\n").as_deref(),
        Some("nightly build")
    );
    assert_eq!(parse_version("  \n"), None);
}

#[test]
fn parses_captured_omo_model_table() {
    let models = parse_omo_models(&fixture("omo-list-models.txt")).expect("parses");
    assert_eq!(models.len(), 18);
    assert_eq!(models[0], "anthropic/claude-fable-5");
    assert!(models.contains(&"alibaba-token-plan/deepseek-v3.2".to_string()));
    assert_eq!(
        models.last().map(String::as_str),
        Some("zai-coding-cn/glm-5.3-highspeed")
    );
}

#[test]
fn omo_table_without_header_is_an_error() {
    assert!(parse_omo_models("error: no providers configured\n").is_err());
    assert_eq!(
        parse_omo_models("provider  model  context\n").expect("header only"),
        Vec::<String>::new()
    );
}

#[test]
fn parses_captured_omp_models_json() {
    let models = parse_omp_models(&fixture("omp-models.json")).expect("parses");
    assert_eq!(
        models,
        [
            "anthropic/claude-fable-5",
            "anthropic/claude-fable-5-1",
            "anthropic/claude-haiku-4-5",
            "anthropic/claude-3-5-sonnet-20240620",
            "anthropic/claude-3-5-sonnet-20241022",
        ]
    );
}

#[test]
fn omp_json_falls_back_to_provider_id_and_rejects_other_shapes() {
    let models =
        parse_omp_models(r#"{"models":[{"provider":"p","id":"m"},{"id":"bare"},{}]}"#).expect("ok");
    assert_eq!(models, ["p/m", "bare"]);
    assert!(parse_omp_models("not json").is_err());
    assert!(parse_omp_models(r#"[{"id":"x"}]"#).is_err());
}

#[test]
fn program_of_takes_the_first_real_word() {
    assert_eq!(
        program_of("omo -n '{name}' {model_args} \"$(cat '{prompt_file}')\"").as_deref(),
        Some("omo")
    );
    assert_eq!(
        program_of("FOO=1 BAR=x '/opt/bin/claude' {model_args}").as_deref(),
        Some("/opt/bin/claude")
    );
    assert_eq!(program_of("{agent} run"), None);
    assert_eq!(program_of("   "), None);
}

fn availability_fixture() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut store = Store::init(tmp.path()).expect("store");
    store.config.sync.omo_agent_dir = Some(tmp.path().join("agent"));
    std::fs::create_dir(tmp.path().join("agent")).expect("agent dir");
    (
        tmp,
        Ctx {
            herdr: corral_herdr::Herdr::with_bin(
                store.root().join("unused-herdr"),
                Some("corral-test".into()),
            ),
            store,
        },
    )
}

#[test]
fn availability_prefers_configured_settings_and_authenticated_providers() {
    // Given a configured directory and both credential shapes, plus unusable records.
    let (tmp, ctx) = availability_fixture();
    std::fs::write(
        tmp.path().join("agent/settings.json"),
        r#"{"enabledModels":["oauth/model","api/model","missing/model","api/model",
            "empty/model","unknown/model"],"unrelated":true}"#,
    )
    .expect("settings");
    std::fs::write(
        tmp.path().join("agent/auth.json"),
        r#"{"api":{"type":"api_key","key":"test-only"},
            "oauth":{"type":"oauth","refresh":"test-only","expires":0},
            "empty":{"type":"api_key","key":""},"unknown":{"type":"other"},
            "missing":null}"#,
    )
    .expect("auth");

    // When routing asks for availability.
    let models = preferred_omo_models(&ctx);

    // Then settings win over discovery, with unique, sorted authenticated selectors.
    assert_eq!(models, Some(vec!["api/model".into(), "oauth/model".into()]));
    assert!(!ctx.store.state_dir().join(HARNESS_CACHE_FILE).exists());
}

#[test]
fn availability_preserves_known_empty_authenticated_intersection() {
    // Given settings whose only provider has no authentication.
    let (tmp, ctx) = availability_fixture();
    std::fs::write(
        tmp.path().join("agent/settings.json"),
        r#"{"enabledModels":["missing/model"]}"#,
    )
    .expect("settings");
    std::fs::write(tmp.path().join("agent/auth.json"), "{}").expect("auth");

    // When availability is requested.
    let models = preferred_omo_models(&ctx);

    // Then an empty known set does not silently fall back to discovery.
    assert_eq!(models, Some(Vec::new()));
}

#[test]
fn availability_uses_enabled_models_when_auth_is_unavailable() {
    for auth in [None, Some("invalid"), Some("[]")] {
        // Given valid settings but absent or malformed auth.
        let (tmp, ctx) = availability_fixture();
        std::fs::write(
            tmp.path().join("agent/settings.json"),
            r#"{"enabledModels":["provider/model"]}"#,
        )
        .expect("settings");
        if let Some(auth) = auth {
            std::fs::write(tmp.path().join("agent/auth.json"), auth).expect("auth");
        }

        // When availability is requested.
        let models = preferred_omo_models(&ctx);

        // Then no ungrounded provider restriction is applied.
        assert_eq!(models, Some(vec!["provider/model".into()]));
    }
}

#[test]
fn availability_falls_back_to_discovery_when_settings_are_unusable() {
    for settings in [
        None,
        Some("invalid"),
        Some("{}"),
        Some(r#"{"enabledModels":[null]}"#),
        Some(r#"{"enabledModels":["bare"]}"#),
        Some(r#"{"enabledModels":["provider/"]}"#),
        Some(r#"{"enabledModels":["provider/*"]}"#),
    ] {
        // Given no concrete enabled model list.
        let (tmp, ctx) = availability_fixture();
        if let Some(settings) = settings {
            std::fs::write(tmp.path().join("agent/settings.json"), settings).expect("settings");
        }

        // When availability is requested.
        let models = preferred_omo_models(&ctx);

        // Then the caller can fall back to the original discovery listing.
        assert_eq!(models, None);
    }
}

#[test]
fn availability_preserves_explicitly_empty_enabled_models() {
    // Given an explicitly empty enabled list without any auth file.
    let (tmp, ctx) = availability_fixture();
    std::fs::write(
        tmp.path().join("agent/settings.json"),
        r#"{"enabledModels":[]}"#,
    )
    .expect("settings");

    // When availability is requested.
    let models = preferred_omo_models(&ctx);

    // Then the caller must not substitute its discovery listing.
    assert_eq!(models, Some(Vec::new()));
}

#[test]
fn missing_binary_is_reported_not_installed() {
    let mut config = Config::default_for(Path::new("/nonexistent"));
    config.harnesses.clear();
    config.harnesses.insert(
        "omo".into(),
        harness("/nonexistent/corral-test/omo {model_args}", None),
    );
    config.harnesses.insert(
        "claude".into(),
        harness("/nonexistent/corral-test/claude {model_args}", None),
    );
    let found = detect_harnesses(&config);
    let omo = found.iter().find(|h| h.key == "omo").expect("omo");
    assert!(!omo.installed());
    assert_eq!(omo.version, None);
    assert_eq!(omo.model_source, ModelSource::FreeText);
    assert!(omo.models.is_empty());
    assert!(omo.takes_effort);
    assert!(omo.efforts.contains(&"xhigh".to_string()));
    let claude = found.iter().find(|h| h.key == "claude").expect("claude");
    assert!(!claude.installed());
    assert_eq!(claude.model_source, ModelSource::Builtin);
    assert_eq!(claude.models, ["opus", "sonnet", "haiku"]);
    assert_eq!(claude.efforts, ["low", "medium", "high", "xhigh", "max"]);
}

#[cfg(unix)]
fn write_script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nexport CORRAL_ROOT=\"$(cd \"$(dirname \"$0\")/../ws\" && pwd)\"\n\
             unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID\n{body}\n"
        ),
    )
    .expect("write script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path.to_string_lossy().into_owned()
}

/// Keep every fake-script write and probe in this one test: a fork from a concurrent test could inherit a
/// script's write fd and make its exec fail with ETXTBSY.
#[cfg(unix)]
#[test]
fn detects_fake_harnesses_degrades_and_caches() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let fake = write_script(
        &bin,
        "fake",
        &format!(
            "case \"$1\" in\n  --version) cat '{FIXTURES}/omo-version.txt' ;;\n  \
             --list-models) cat '{FIXTURES}/omo-list-models.txt' ;;\n  \
             models) cat '{FIXTURES}/omp-models.json' ;;\n  *) exit 2 ;;\nesac"
        ),
    );
    let garbage = write_script(&bin, "garbage", "echo boom");
    let failing = write_script(&bin, "failing", "exit 3");

    let root = tmp.path().join("ws");
    std::fs::create_dir_all(&root).expect("ws dir");
    let mut store = Store::init(&root).expect("init store");
    let harnesses = &mut store.config.harnesses;
    harnesses.clear();
    harnesses.insert(
        "omo".into(),
        harness(&format!("{fake} {{model_args}}"), None),
    );
    harnesses.insert(
        "omp".into(),
        harness(&format!("'{fake}' {{model_args}}"), Some("omp")),
    );
    harnesses.insert(
        "weird".into(),
        harness(&format!("{garbage} {{model_args}}"), Some("omo")),
    );
    harnesses.insert(
        "dead".into(),
        harness(&format!("{failing} {{model_args}}"), Some("omp")),
    );
    harnesses.insert("generic".into(), harness(&format!("{garbage} x"), None));
    let ctx = Ctx {
        herdr: corral_herdr::Herdr::with_bin(
            store.root().join("unused-herdr"),
            Some("corral-test".into()),
        ),
        store,
    };

    let report = harness_report(&ctx, false).expect("report");
    let keys: Vec<&str> = report.harnesses.iter().map(|h| h.key.as_str()).collect();
    assert_eq!(keys, ["dead", "generic", "omo", "omp", "weird"]);

    let omo = report.get("omo").expect("omo");
    assert!(omo.installed());
    assert_eq!(omo.version.as_deref(), Some("5.1.33"));
    assert_eq!(omo.model_source, ModelSource::Listed);
    assert_eq!(omo.models.len(), 18);
    assert!(omo.notes.is_empty(), "{:?}", omo.notes);

    let omp = report.get("omp").expect("omp");
    assert_eq!(omp.model_source, ModelSource::Listed);
    assert_eq!(omp.models.len(), 5);
    assert!(omp.efforts.contains(&"auto".to_string()));

    let weird = report.get("weird").expect("weird");
    assert!(weird.installed());
    assert_eq!(weird.version.as_deref(), Some("boom"));
    assert_eq!(weird.model_source, ModelSource::FreeText);
    assert!(weird.notes.iter().any(|n| n.contains("--list-models")));

    let dead = report.get("dead").expect("dead");
    assert!(dead.installed());
    assert_eq!(dead.version, None);
    assert_eq!(dead.model_source, ModelSource::FreeText);
    assert_eq!(dead.notes.len(), 2, "{:?}", dead.notes);

    let generic = report.get("generic").expect("generic");
    assert_eq!(generic.model_source, ModelSource::None);
    assert!(!generic.takes_effort);

    let cache = ctx.store.state_dir().join(HARNESS_CACHE_FILE);
    let mut cached: HarnessReport =
        serde_json::from_str(&std::fs::read_to_string(&cache).expect("cache")).expect("json");
    assert_eq!(cached, report);
    cached.harnesses[2].version = Some("tampered".into());
    std::fs::write(&cache, serde_json::to_string(&cached).expect("ser")).expect("write");

    let again = harness_report(&ctx, false).expect("cached");
    assert_eq!(
        again.get("omo").and_then(|h| h.version.as_deref()),
        Some("tampered")
    );
    let fresh = harness_report(&ctx, true).expect("refresh");
    assert_eq!(
        fresh.get("omo").and_then(|h| h.version.as_deref()),
        Some("5.1.33")
    );

    let mut moved = ctx.clone();
    moved.store.config.harnesses.insert(
        "omo".into(),
        harness("/nonexistent/corral-test/omo {model_args}", None),
    );
    let redetected = harness_report(&moved, false).expect("program changed");
    assert!(!redetected.get("omo").expect("omo").installed());
}

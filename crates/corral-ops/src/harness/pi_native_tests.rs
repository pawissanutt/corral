use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::agent::bridge::assets::HOST_BINARY;
use crate::agent::install::tests::package;

const LISTING: &str = r#"{"models":[
  {"selector":"openai-codex/gpt-6.1-sol","provider":"openai-codex","id":"gpt-6.1-sol"},
  {"selector":"openai-codex/gpt-5.5","provider":"openai-codex","id":"gpt-5.5"},
  {"selector":"anthropic/claude-opus-5-5","provider":"anthropic","id":"claude-opus-5-5"},
  {"selector":"amazon-bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0","provider":"amazon-bedrock","id":"us.anthropic.claude-sonnet-4-5-20250929-v1:0"}
]}"#;

/// A stand-in `bun` that installs like `install::tests::fake_bun` and answers the model listing
/// with `LISTING`, recording that call's argv in `<dir>/listing-argv`.
fn listing_bun(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("listing.json"), LISTING).unwrap();
    let source = dir.join("bun.sh");
    std::fs::write(
        &source,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 1.4.2 ;;\ninstall) mkdir -p node_modules ;;\n\
             --no-env-file) echo \"$*\" > {dir}/listing-argv; cat {dir}/listing.json ;;\n\
             *) exit 9 ;;\nesac\n",
            dir = dir.display()
        ),
    )
    .unwrap();
    let bun = dir.join("bun");
    let installed = std::process::Command::new("install")
        .args(["-m", "0755"])
        .arg(&source)
        .arg(&bun)
        .status()
        .unwrap();
    assert!(installed.success());
    bun
}

fn login(root: &Path, provider: &str) {
    let dir = credentials::auth_dir(root);
    credentials::ensure_auth_dir(&dir).unwrap();
    let file = dir.join(format!("{provider}.json"));
    std::fs::write(&file, "{\"type\":\"oauth\"}").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn config() -> Config {
    Config::default_for(Path::new("/nonexistent"))
}

#[test]
fn lists_only_logged_in_providers_from_the_installed_host() {
    // Given the real package installed with a listing stand-in and only OpenAI logged in:
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let bun = listing_bun(&tmp.path().join("bin"));
    let record = install::install_into(&package(), &root, Some(bun.clone())).unwrap();
    assert!(record.path.join(MODELS_ENTRY).is_file());
    login(&root, "openai-codex");

    // When pi-native is detected:
    let info = detect_with(&config(), Some(root), Some(bun.clone()));

    // Then its models are the listed openai-codex selectors, and only that provider was asked for:
    assert_eq!(info.model_source, ModelSource::Listed, "{:?}", info.notes);
    assert_eq!(
        info.models,
        vec!["openai-codex/gpt-6.1-sol", "openai-codex/gpt-5.5"]
    );
    assert!(info.installed() && info.notes.is_empty());
    assert_eq!(info.version.as_deref(), Some("1.3.0"));
    assert!(info.takes_effort && info.efforts.contains(&"medium".to_owned()));
    let argv = std::fs::read_to_string(tmp.path().join("bin/listing-argv")).unwrap();
    assert!(
        argv.trim()
            .ends_with("src/auth/models.ts --provider openai-codex"),
        "{argv}"
    );
}

#[test]
fn degrades_to_free_text_naming_the_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let bun = listing_bun(&tmp.path().join("bin"));
    let note = |root: &Path, bun: Option<PathBuf>| {
        let info = detect_with(&config(), Some(root.to_path_buf()), bun);
        assert_eq!(info.model_source, ModelSource::FreeText);
        assert!(info.models.is_empty());
        info.notes.join("; ")
    };
    // Without an installation the note names `corral agent install`, and no listing ran:
    let missing = note(&root, Some(bun.clone()));
    assert!(missing.contains("run `corral agent install`"), "{missing}");
    assert!(!tmp.path().join("bin/listing-argv").exists());
    // Installed but not logged in, the note names `corral agent login`:
    install::install_into(&package(), &root, Some(bun.clone())).unwrap();
    let unlogged = note(&root, Some(bun.clone()));
    assert!(unlogged.contains("run `corral agent login`"), "{unlogged}");
    // A loose credential file is not a login doctor accepts:
    login(&root, "openai-codex");
    std::fs::set_permissions(
        credentials::auth_dir(&root).join("openai-codex.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(note(&root, Some(bun)).contains("corral agent login"));
    assert!(!tmp.path().join("bin/listing-argv").exists());
    // Without Bun nothing can list:
    assert!(note(&root, None).contains("no executable `bun`"));
}

/// A stand-in compiled `corral-pi-host`: `--version` and `--write-install` like
/// src/host/compiled.ts, and `models` answering with `LISTING` and recording its argv.
fn listing_host(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("listing.json"), LISTING).unwrap();
    let source = dir.join("host.sh");
    std::fs::write(
        &source,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 'corral-pi-host pi 1.3.0 bun 1.4.2' ;;\n\
             --write-install) cp '{compat}' \"$2/compat.json\" ;;\n\
             models) echo \"$*\" > {dir}/listing-argv; cat {dir}/listing.json ;;\n\
             *) exit 9 ;;\nesac\n",
            compat = package().join("compat.json").display(),
            dir = dir.display()
        ),
    )
    .unwrap();
    let host = dir.join(HOST_BINARY);
    let installed = std::process::Command::new("install")
        .args(["-m", "0755"])
        .arg(&source)
        .arg(&host)
        .status()
        .unwrap();
    assert!(installed.success());
    host
}

#[test]
fn a_compiled_installation_lists_through_its_host_without_bun() {
    // Given a compiled installation, no Bun anywhere, and only OpenAI logged in:
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let host = listing_host(&tmp.path().join("release"));
    let record = install::install_compiled(&host, &root).unwrap();
    login(&root, "openai-codex");

    // When pi-native is detected:
    let info = detect_with(&config(), Some(root), None);

    // Then the installed host listed the openai-codex models through its `models` mode:
    assert_eq!(info.model_source, ModelSource::Listed, "{:?}", info.notes);
    assert_eq!(
        info.models,
        vec!["openai-codex/gpt-6.1-sol", "openai-codex/gpt-5.5"]
    );
    assert_eq!(info.path, Some(record.path.join(HOST_BINARY)));
    let argv = std::fs::read_to_string(tmp.path().join("release/listing-argv")).unwrap();
    assert_eq!(argv.trim(), "models --provider openai-codex");
}

#[test]
fn a_profile_sees_and_accepts_only_its_own_providers_models() {
    // Given a catalog listing both providers' models:
    let models: Vec<String> = ["openai-codex/gpt-6.1-sol", "anthropic/claude-opus-5-5"]
        .map(String::from)
        .to_vec();
    let scoped = |vendor, model| models_for_profile("pi-native", vendor, model, &models);
    // Then each profile sees only its provider, from its vendor or else its model:
    assert_eq!(scoped(Some("openai"), None), ["openai-codex/gpt-6.1-sol"]);
    assert_eq!(
        scoped(None, Some("claude-opus-5-5")),
        ["anthropic/claude-opus-5-5"]
    );
    assert_eq!(scoped(None, None), models, "no provider: the whole list");
    assert_eq!(
        models_for_profile("omo", Some("openai"), None, &models),
        models
    );
    // And a bare id is looked up qualified by the bound provider, a qualified one as is:
    assert_eq!(
        catalog_selector("pi-native", Some("openai"), "gpt-6.1-sol"),
        "openai-codex/gpt-6.1-sol"
    );
    assert_eq!(
        catalog_selector("pi-native", None, "claude-opus-5-5"),
        "anthropic/claude-opus-5-5"
    );
    assert_eq!(
        catalog_selector("omo", Some("openai"), "gpt-6.1-sol"),
        "gpt-6.1-sol"
    );
    // And another provider's selector is foreign to a profile bound elsewhere:
    assert!(foreign_selector(
        "pi-native",
        Some("openai"),
        "anthropic/claude-opus-5-5"
    ));
    assert!(foreign_selector(
        "pi-native",
        Some("anthropic"),
        "openai-codex/gpt-6.1-sol"
    ));
    assert!(!foreign_selector(
        "pi-native",
        Some("openai"),
        "openai-codex/gpt-6.1-sol"
    ));
    assert!(!foreign_selector(
        "pi-native",
        None,
        "anthropic/claude-opus-5-5"
    ));
}

#[test]
fn bedrock_listing_requires_a_valid_named_profile_binding() {
    // Given a host declaring Bedrock, and a valid subscription login.
    for binding in [
        None,
        Some(r#"{"type":"oauth"}"#),
        Some(r#"{"type":"aws_profile","profile":"dev","region":"us-west-2"}"#),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let host = listing_host(&tmp.path().join("release"));
        install::install_compiled(&host, &root).unwrap();
        login(&root, "openai-codex");
        if let Some(binding) = binding {
            let path = credentials::auth_dir(&root).join("amazon-bedrock.json");
            std::fs::write(&path, binding).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        // When the offline catalog is requested.
        let info = detect_with(&config(), Some(root), None);

        // Then only a validated binding admits Bedrock to the request and returned selectors.
        let bound = binding.is_some_and(|b| b.contains("aws_profile"));
        assert_eq!(info.model_source, ModelSource::Listed, "{:?}", info.notes);
        let argv = std::fs::read_to_string(tmp.path().join("release/listing-argv")).unwrap();
        assert_eq!(argv.contains("--provider amazon-bedrock"), bound, "{argv}");
        let scoped = models_for_profile("pi-native", Some("amazon-bedrock"), None, &info.models);
        assert_eq!(
            scoped,
            if bound {
                vec!["amazon-bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0"]
            } else {
                Vec::new()
            }
        );
    }
}

#[test]
fn bedrock_selectors_bind_to_bedrock_even_for_anthropic_models() {
    // Given a bare Bedrock ID with an Anthropic publisher.
    let model = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
    let selector = format!("amazon-bedrock/{model}");
    let models = vec![selector.clone(), "anthropic/claude-sonnet-4-5".into()];
    // When catalog selection runs, then it uses the transport vendor, not the publisher.
    assert_eq!(
        models_for_profile("pi-native", None, Some(model), &models),
        [selector.clone()]
    );
    assert_eq!(
        catalog_selector("pi-native", Some("amazon-bedrock"), model),
        selector
    );
    assert!(!foreign_selector(
        "pi-native",
        Some("amazon-bedrock"),
        &selector
    ));
    assert!(foreign_selector(
        "pi-native",
        Some("amazon-bedrock"),
        "anthropic/claude-sonnet-4-5"
    ));
}

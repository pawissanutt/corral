use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::agent::install::tests::package;

const LISTING: &str = r#"{"models":[
  {"selector":"openai-codex/gpt-6.1-sol","provider":"openai-codex","id":"gpt-6.1-sol"},
  {"selector":"openai-codex/gpt-5.5","provider":"openai-codex","id":"gpt-5.5"},
  {"selector":"anthropic/claude-opus-5-5","provider":"anthropic","id":"claude-opus-5-5"}
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

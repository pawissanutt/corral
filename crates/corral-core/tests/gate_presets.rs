use std::fs;
use std::path::Path;

use corral_core::detect_gate_presets;

fn file(repo: &Path, name: &str, text: &str) {
    let path = repo.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn repository_and_just_gates_precede_stack_defaults_without_execution() {
    let repo = tempfile::tempdir().unwrap();
    let script = "#!/bin/sh\ntouch was-executed\n";
    let justfile = "# gate: ignored\ncheck:\n    cargo test\ngate: check\n";
    file(repo.path(), "scripts/gate.sh", script);
    file(repo.path(), "justfile", justfile);
    file(repo.path(), "Cargo.toml", "[workspace]\nmembers = []");
    file(repo.path(), ".config/nextest.toml", "[profile.default]\n");
    let presets = detect_gate_presets(repo.path()).unwrap();
    assert_eq!(presets[0].commands, ["bash scripts/gate.sh"]);
    assert_eq!(presets[0].preview.as_deref(), Some(script));
    assert_eq!(presets[1].commands, ["just gate"]);
    assert_eq!(presets[1].preview.as_deref(), Some(justfile));
    assert_eq!(presets[2].commands, ["just check"]);
    assert_eq!(
        presets[3].commands,
        [
            "cargo fmt --all -- --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test",
        ]
    );
    assert_eq!(presets[4].commands.last().unwrap(), "cargo nextest run");
    assert!(!repo.path().join("was-executed").exists());
}

#[test]
fn javascript_uses_lockfile_manager_and_only_supported_existing_scripts() {
    for (lockfile, manager) in [
        ("pnpm-lock.yaml", "pnpm"),
        ("package-lock.json", "npm"),
        ("npm-shrinkwrap.json", "npm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
        ("no-lockfile", "npm"),
    ] {
        let repo = tempfile::tempdir().unwrap();
        file(repo.path(), lockfile, "");
        file(
            repo.path(),
            "package.json",
            r#"{
            "scripts": {
                "dev": "server", "test": "vitest run", "lint": "eslint .",
                "check": "", "typecheck": false, "build": "vite build"
            }
        }"#,
        );
        let presets = detect_gate_presets(repo.path()).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(
            presets[0].commands,
            [
                format!("{manager} run lint"),
                format!("{manager} run test"),
                format!("{manager} run build"),
            ]
        );
    }
}

#[test]
fn python_uv_ruff_and_go_are_independent_suggestions() {
    let repo = tempfile::tempdir().unwrap();
    file(
        repo.path(),
        "pyproject.toml",
        "[tool.ruff]\nline-length = 100\n",
    );
    file(repo.path(), "uv.lock", "");
    file(repo.path(), "go.mod", "module example.com/fixture\n");
    let presets = detect_gate_presets(repo.path()).unwrap();
    assert_eq!(presets.len(), 2);
    assert_eq!(
        presets[0].commands,
        [
            "uv run ruff format --check .",
            "uv run ruff check .",
            "uv run pytest"
        ]
    );
    assert_eq!(presets[1].commands, ["go vet ./...", "go test ./..."]);
}

#[test]
fn plain_pytest_does_not_assume_uv_or_ruff() {
    let repo = tempfile::tempdir().unwrap();
    file(repo.path(), "pytest.ini", "[pytest]\n");
    let presets = detect_gate_presets(repo.path()).unwrap();
    assert_eq!(presets[0].commands, ["pytest"]);
}

#[test]
fn absent_stack_and_unrelated_just_variables_do_not_produce_suggestions() {
    let repo = tempfile::tempdir().unwrap();
    file(
        repo.path(),
        "Justfile",
        "check := 'variable'\n# gate:\nother:\n    echo check:\n",
    );
    file(
        repo.path(),
        "package.json",
        r#"{"scripts":{"start":"server"}}"#,
    );
    assert!(detect_gate_presets(repo.path()).unwrap().is_empty());
}

#[test]
fn oversized_and_malformed_manifests_report_errors() {
    let repo = tempfile::tempdir().unwrap();
    file(repo.path(), "package.json", "not json");
    assert!(detect_gate_presets(repo.path()).is_err());
    file(repo.path(), "package.json", &" ".repeat(128 * 1024 + 1));
    assert!(detect_gate_presets(repo.path()).is_err());
}

#[test]
fn script_preview_is_bounded_on_utf8_boundaries() {
    let repo = tempfile::tempdir().unwrap();
    file(repo.path(), "scripts/gate.sh", &"é".repeat(20 * 1024));
    let presets = detect_gate_presets(repo.path()).unwrap();
    let preview = presets[0].preview.as_ref().unwrap();
    assert!(preview.len() < 17 * 1024);
    assert!(preview.starts_with("é"));
}

#[cfg(unix)]
#[test]
fn special_files_and_symlinks_are_not_read() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join("scripts")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", repo.path().join("scripts/gate.sh")).unwrap();
    assert!(detect_gate_presets(repo.path()).unwrap().is_empty());
}

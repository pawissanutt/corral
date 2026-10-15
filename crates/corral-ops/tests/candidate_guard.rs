//! Candidate size policy exercised through the real read-only Python guard.
use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};

mod support;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = support::command("git", repo)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn fixture() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["config", "user.name", "Size guard"]);
    git(tmp.path(), &["config", "user.email", "guard@test.invalid"]);
    git(tmp.path(), &["commit", "-qm", "base", "--allow-empty"]);
    let base = git(tmp.path(), &["rev-parse", "HEAD"]);
    (tmp, base)
}

fn guard(repo: &Path, base: &str, limits: Option<&Value>, env_limits: Option<&Value>) -> Value {
    let mut command = support::command("python3", repo);
    command
        .current_dir(repo)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-candidate.py"))
        .args(["--base", base])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .env_remove("CORRAL_SIZE_LIMITS");
    if let Some(limits) = limits {
        command.args(["--size-limits", &limits.to_string()]);
    }
    if let Some(limits) = env_limits {
        command.env("CORRAL_SIZE_LIMITS", limits.to_string());
    }
    let output = command.output().unwrap();
    assert!(output.stderr.is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.success(), report["ok"].as_bool().unwrap());
    report
}

#[test]
fn configured_rust_and_typescript_boundaries_are_both_enforced() {
    // Given custom Rust and TypeScript boundaries, with ignored blank/comment lines.
    let (tmp, base) = fixture();
    for (path, count) in [
        ("pass.rs", 1000),
        ("fail.rs", 1001),
        ("pass.ts", 400),
        ("fail.ts", 401),
    ] {
        std::fs::write(
            tmp.path().join(path),
            format!("// ignored\n\n{}", "call();\n".repeat(count)),
        )
        .unwrap();
    }
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-qm", "sources"]);

    // When the real candidate guard runs with the effective extension map.
    let report = guard(tmp.path(), &base, Some(&json!({"rs":1000,"ts":400})), None);

    // Then both exact boundaries pass and both excesses are reported.
    assert_eq!(report["counts"]["source_files"], 4);
    assert_eq!(report["violations"].as_array().unwrap().len(), 2);
    assert_eq!(report["violations"][0]["path"], "fail.rs");
    assert_eq!(report["violations"][0]["detail"], "1001 > 1000");
    assert_eq!(report["violations"][1]["path"], "fail.ts");
    assert_eq!(report["violations"][1]["detail"], "401 > 400");
}

#[test]
fn missing_configuration_preserves_rust_eight_hundred_default() {
    // Given one Rust file above the default and an unconfigured TypeScript file.
    let (tmp, base) = fixture();
    std::fs::write(tmp.path().join("large.rs"), "call();\n".repeat(801)).unwrap();
    std::fs::write(tmp.path().join("large.ts"), "call();\n".repeat(900)).unwrap();
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-qm", "sources"]);

    // When no size map is supplied.
    let report = guard(tmp.path(), &base, None, None);

    // Then only Rust's built-in 800 ceiling applies.
    assert_eq!(report["counts"]["source_files"], 1);
    assert_eq!(report["violations"][0]["detail"], "801 > 800");
}

#[test]
fn explicit_limits_override_gate_environment_limits() {
    // Given a gate environment with a lower limit than the explicit command.
    let (tmp, base) = fixture();
    std::fs::write(tmp.path().join("source.rs"), "call();\n".repeat(900)).unwrap();
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-qm", "source"]);

    // When the explicit map overrides the environment.
    let report = guard(
        tmp.path(),
        &base,
        Some(&json!({"rs":1000})),
        Some(&json!({"rs":800})),
    );

    // Then the explicit limit is used.
    assert_eq!(report["ok"], true);
    assert_eq!(report["source_files"][0]["limit"], 1000);
}

#[test]
fn gate_environment_supplies_all_configured_extension_limits() {
    // Given a non-Rust source and a custom extension supplied by the gate environment.
    let (tmp, base) = fixture();
    std::fs::write(tmp.path().join("source.custom"), "call();\n".repeat(3)).unwrap();
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-qm", "source"]);

    // When the guard receives no command-line override.
    let report = guard(
        tmp.path(),
        &base,
        None,
        Some(&json!({"rs":1000,"custom":2})),
    );

    // Then it evaluates the configured extension rather than only Rust.
    assert_eq!(report["violations"][0]["path"], "source.custom");
    assert_eq!(report["violations"][0]["detail"], "3 > 2");
}

#[test]
fn renamed_sources_keep_each_configured_extension_policy() {
    // Given a Rust file renamed to TypeScript with distinct ceilings.
    let (tmp, _) = fixture();
    std::fs::write(tmp.path().join("source.rs"), "call();\n".repeat(500)).unwrap();
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-qm", "source"]);
    let base = git(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["mv", "source.rs", "source.ts"]);
    git(tmp.path(), &["commit", "-qm", "rename"]);

    // When both policy paths are inspected.
    let report = guard(tmp.path(), &base, Some(&json!({"rs":1000,"ts":400})), None);

    // Then renaming cannot evade the stricter extension policy.
    assert_eq!(report["counts"]["source_files"], 2);
    assert_eq!(report["violations"][0]["path"], "source.ts");
    assert_eq!(report["violations"][0]["detail"], "500 > 400");
}

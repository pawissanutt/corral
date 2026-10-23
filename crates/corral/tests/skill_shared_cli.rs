//! `corral skill install` and `corral doctor` against a fake HOME that has both OmO skill directories.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_ops::{ROLE_SKILLS, SKILL_MARKER};
use serde_json::Value;

fn corral(home: &Path, args: &[&str]) -> Output {
    let root = home.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .arg("--root")
        .arg(&root)
        .args(args)
        .current_dir(&root)
        .env_remove("CORRAL_ROOT")
        .env_remove("HERDR_ENV")
        .env_remove("CORRAL_HERDR_SESSION")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("PATH", home.join("empty-bin"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn fake_home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for dir in [".omo/agent/skills", ".agents/skills"] {
        std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    tmp
}

fn skill_file(home: &Path, dir: &str, role: &str) -> std::path::PathBuf {
    home.join(dir).join(role).join("SKILL.md")
}

fn duplicate_check(out: &Output) -> Value {
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "skill_duplicates")
        .cloned()
        .unwrap()
}

#[test]
fn skill_install_writes_one_copy_and_removes_an_owned_duplicate() {
    let tmp = fake_home();
    let home = tmp.path();
    let owned = corral_ops::role_skill_markdown("corral").unwrap();
    std::fs::create_dir_all(home.join(".omo/agent/skills/corral")).unwrap();
    std::fs::write(skill_file(home, ".omo/agent/skills", "corral"), &owned).unwrap();
    std::fs::create_dir_all(home.join(".omo/agent/skills/mine")).unwrap();
    std::fs::write(skill_file(home, ".omo/agent/skills", "mine"), "not corral").unwrap();

    let out = corral(home, &["skill", "install"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("removed duplicate"), "{stdout}");
    for role in ROLE_SKILLS {
        assert!(skill_file(home, ".agents/skills", role).is_file(), "{role}");
        assert!(
            !skill_file(home, ".omo/agent/skills", role).exists(),
            "{role}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(skill_file(home, ".omo/agent/skills", "mine")).unwrap(),
        "not corral"
    );
    let again = corral(home, &["skill", "install"]);
    assert!(!String::from_utf8_lossy(&again.stdout).contains("wrote "));
}

#[test]
fn doctor_reports_a_duplicate_and_fix_repairs_it() {
    let tmp = fake_home();
    let home = tmp.path();
    let dup = skill_file(home, ".omo/agent/skills", "corral-worker");
    std::fs::create_dir_all(dup.parent().unwrap()).unwrap();
    std::fs::write(&dup, format!("{SKILL_MARKER}\nold copy\n")).unwrap();

    let before = duplicate_check(&corral(home, &["doctor", "--json"]));
    assert_eq!(before["ok"], false, "{before}");
    assert!(
        before["message"]
            .as_str()
            .unwrap()
            .contains("corral-worker")
    );
    assert!(before["fix"].as_str().unwrap().contains("doctor --fix"));

    corral(home, &["doctor", "--fix", "--json"]);
    assert!(!dup.exists());
    let after = duplicate_check(&corral(home, &["doctor", "--json"]));
    assert_eq!(after["ok"], true, "{after}");
    for role in ROLE_SKILLS {
        assert!(skill_file(home, ".agents/skills", role).is_file(), "{role}");
    }
}

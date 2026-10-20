use super::*;

#[test]
fn skill_install_writes_is_idempotent_and_protects_foreign_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dir = root.join("skills");
    let dir_s = dir.to_string_lossy().into_owned();
    let printed = ok(root, &["skill"]);
    assert!(printed.starts_with("---\nname: corral\n"));

    let wrote = ok(root, &["skill", "install", "--dir", &dir_s]);
    for role in corral_ops::ROLE_SKILLS {
        assert!(wrote.contains(&format!("dir/{role}: wrote ")), "{wrote}");
        assert!(dir.join(role).join("SKILL.md").is_file(), "{role}");
    }
    let file = dir.join("corral").join("SKILL.md");
    assert_eq!(
        std::fs::read_to_string(&file).expect("skill written"),
        printed
    );
    let current = ok(root, &["skill", "install", "--dir", &dir_s]);
    assert_eq!(current.matches(": up to date ").count(), 4, "{current}");
    let worker = dir.join(corral_ops::WORKER_SKILL).join("SKILL.md");
    std::fs::write(&worker, format!("{}\nold\n", corral_ops::SKILL_MARKER)).expect("stale");
    let refreshed = ok(root, &["skill", "install", "--dir", &dir_s]);
    assert!(
        refreshed.contains(&format!("dir/{}: refreshed ", corral_ops::WORKER_SKILL)),
        "{refreshed}"
    );

    // Mixed outcome: foreign PM file, stale worker, missing planner, current submit skill.
    std::fs::write(&file, "my own skill").expect("write foreign");
    std::fs::write(&worker, format!("{}\nold\n", corral_ops::SKILL_MARKER)).expect("stale");
    let planner = dir.join(corral_ops::PLANNER_SKILL).join("SKILL.md");
    std::fs::remove_file(&planner).expect("remove planner");
    let mixed = corral(root, &["skill", "install", "--dir", &dir_s]);
    assert!(!mixed.status.success());
    let stdout = String::from_utf8_lossy(&mixed.stdout);
    let stderr = String::from_utf8_lossy(&mixed.stderr);
    for (role, action) in [
        (corral_ops::WORKER_SKILL, "refreshed"),
        (corral_ops::PLANNER_SKILL, "wrote"),
        (corral_ops::SUBMIT_SKILL, "up to date"),
    ] {
        assert!(
            stdout.contains(&format!("dir/{role}: {action} ")),
            "{role}: {stdout}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(role).join("SKILL.md")).ok(),
            corral_ops::role_skill_markdown(role),
            "{role}"
        );
    }
    assert!(
        stderr.contains("dir/corral: skipped (not written by corral) "),
        "{stderr}"
    );
    assert!(
        stderr.contains("skill not installed for: dir/corral"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("kept"),
        "my own skill"
    );
    ok(root, &["skill", "install", "--dir", &dir_s, "--force"]);
    assert_eq!(std::fs::read_to_string(&file).expect("replaced"), printed);
}

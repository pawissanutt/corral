use super::skill::*;

#[test]
fn pi_pack_is_isolated_refreshable_and_preserves_owner_files() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".agents")).unwrap();
    std::fs::create_dir_all(home.path().join(".omo")).unwrap();
    let writes = install_harness_skills(home.path(), "pi-native", false).unwrap();
    assert_eq!(writes.len(), 5);
    let root = home.path().join(".corral/skills/pi-native");
    for name in PI_SKILLS {
        let text = std::fs::read_to_string(root.join(name).join("SKILL.md")).unwrap();
        assert_eq!(text, role_skill_markdown(name).unwrap());
        assert!(text.contains("version="));
        assert!(text.contains("sha256="));
    }
    let notes = root.join("frontend/notes.md");
    std::fs::write(&notes, "owner notes").unwrap();
    let stale = root.join("frontend/SKILL.md");
    std::fs::write(&stale, format!("{SKILL_MARKER}\nstale")).unwrap();
    let foreign = root.join("debugging/SKILL.md");
    std::fs::write(&foreign, "owner skill").unwrap();
    let again = install_harness_skills(home.path(), "pi-native", false).unwrap();
    assert_eq!(
        again.iter().find(|w| w.role == "frontend").unwrap().action,
        SkillAction::Refreshed
    );
    assert_eq!(
        again.iter().find(|w| w.role == "debugging").unwrap().action,
        SkillAction::Unmanaged
    );
    assert_eq!(std::fs::read_to_string(notes).unwrap(), "owner notes");
    assert_eq!(std::fs::read_to_string(foreign).unwrap(), "owner skill");
    install_harness_skills(home.path(), "omo", false).unwrap();
    for name in PI_SKILLS {
        assert!(!home.path().join(".agents/skills").join(name).exists());
        assert!(!home.path().join(".omo/agent/skills").join(name).exists());
    }
    assert!(duplicate_skills(home.path(), "omo").is_empty());
    assert!(
        ensure_skills(home.path(), "pi-native")
            .unwrap()
            .iter()
            .any(|w| w.action == SkillAction::Current)
    );
}

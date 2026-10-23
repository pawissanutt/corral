//! OmO loads `~/.agents/skills` as well as `~/.omo/agent/skills`; corral must keep one copy per skill.

use std::path::{Path, PathBuf};

use corral_ops::{ROLE_SKILLS, SKILL_MARKER, SkillAction, WORKER_SKILL};

fn home_with(markers: &[&str]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for marker in markers {
        std::fs::create_dir_all(home.path().join(marker)).unwrap();
    }
    home
}

fn omo(home: &Path, role: &str) -> PathBuf {
    home.join(".omo/agent/skills").join(role).join("SKILL.md")
}

fn agents(home: &Path, role: &str) -> PathBuf {
    home.join(".agents/skills").join(role).join("SKILL.md")
}

fn put(path: &Path, text: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn omo_installs_only_into_the_shared_dir_when_both_exist() {
    let home = home_with(&[".omo", ".agents"]);
    let home = home.path();
    let writes = corral_ops::ensure_skills(home, "omo").unwrap();
    assert_eq!(writes.len(), ROLE_SKILLS.len());
    for role in ROLE_SKILLS {
        assert!(agents(home, role).is_file(), "{role}");
        assert!(!omo(home, role).exists(), "{role}");
    }
    // The agents harness is already satisfied, and nothing reappears under ~/.omo.
    let again = corral_ops::ensure_skills(home, "agents").unwrap();
    assert!(again.iter().all(|w| w.action == SkillAction::Current));
    assert!(!home.join(".omo/agent").exists());
}

#[test]
fn omo_alone_keeps_its_own_dir() {
    let home = home_with(&[".omo"]);
    let home = home.path();
    corral_ops::ensure_skills(home, "omo").unwrap();
    for role in ROLE_SKILLS {
        assert!(omo(home, role).is_file(), "{role}");
        assert!(!home.join(".agents").exists());
    }
}

#[test]
fn existing_owned_duplicates_are_removed_and_foreign_files_never_touched() {
    let home = home_with(&[".omo", ".agents"]);
    let home = home.path();
    // Identical copy, a stale marker-bearing copy, a foreign file, and an untouched role.
    let current = corral_ops::role_skill_markdown("corral").unwrap();
    put(&omo(home, "corral"), current.as_bytes());
    let stale = format!("{SKILL_MARKER}\nold\n");
    put(&omo(home, WORKER_SKILL), stale.as_bytes());
    put(&omo(home, "corral-planner"), b"my own \xff planner");
    put(&home.join(".omo/agent/skills/corral/notes.txt"), b"keep");

    assert_eq!(corral_ops::duplicate_skills(home, "omo").len(), 2);
    let writes = corral_ops::install_harness_skills(home, "omo", false).unwrap();

    let removed: Vec<_> = writes
        .iter()
        .filter(|w| w.action == SkillAction::Removed)
        .map(|w| w.role)
        .collect();
    assert_eq!(removed, vec!["corral", WORKER_SKILL]);
    assert!(!omo(home, "corral").exists());
    assert!(!omo(home, WORKER_SKILL).exists());
    assert!(!home.join(".omo/agent/skills").join(WORKER_SKILL).exists());
    // A directory holding something else survives, as does the foreign skill.
    assert!(home.join(".omo/agent/skills/corral/notes.txt").is_file());
    assert_eq!(
        std::fs::read(omo(home, "corral-planner")).unwrap(),
        b"my own \xff planner"
    );
    assert!(corral_ops::duplicate_skills(home, "omo").is_empty());
    for role in ROLE_SKILLS {
        assert_eq!(
            std::fs::read_to_string(agents(home, role)).ok(),
            corral_ops::role_skill_markdown(role),
            "{role}"
        );
    }
}

#[test]
fn an_unmanaged_shared_copy_keeps_the_owned_copy_until_forced() {
    let home = home_with(&[".omo", ".agents"]);
    let home = home.path();
    put(&agents(home, "corral"), b"user skill");
    put(
        &omo(home, "corral"),
        corral_ops::role_skill_markdown("corral")
            .unwrap()
            .as_bytes(),
    );

    let writes = corral_ops::install_harness_skills(home, "omo", false).unwrap();
    assert!(
        writes
            .iter()
            .any(|w| w.role == "corral" && w.action == SkillAction::Unmanaged)
    );
    assert!(omo(home, "corral").is_file());
    assert_eq!(
        std::fs::read(agents(home, "corral")).unwrap(),
        b"user skill"
    );

    corral_ops::install_harness_skills(home, "omo", true).unwrap();
    assert!(!omo(home, "corral").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_role_dir_is_not_followed() {
    let home = home_with(&[".omo", ".agents"]);
    let home = home.path();
    let elsewhere = home.join("elsewhere");
    put(
        &elsewhere.join("SKILL.md"),
        corral_ops::role_skill_markdown("corral")
            .unwrap()
            .as_bytes(),
    );
    std::fs::create_dir_all(home.join(".omo/agent/skills")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.join(".omo/agent/skills/corral")).unwrap();

    corral_ops::install_harness_skills(home, "omo", false).unwrap();
    assert!(elsewhere.join("SKILL.md").is_file());
    assert!(corral_ops::duplicate_skills(home, "omo").is_empty());
}

fn alias_omo_to_agents(home: &Path) {
    std::fs::create_dir_all(home.join(".agents/skills")).unwrap();
    std::fs::create_dir_all(home.join(".omo/agent")).unwrap();
    std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".omo/agent/skills"))
        .unwrap();
}

fn assert_all_roles_present(home: &Path) {
    for role in ROLE_SKILLS {
        assert_eq!(
            std::fs::read_to_string(agents(home, role)).ok(),
            corral_ops::role_skill_markdown(role),
            "{role}"
        );
    }
}

#[cfg(unix)]
#[test]
fn aliased_roots_keep_every_skill_for_install_and_ensure() {
    for (explicit, harness) in [(true, "omo"), (false, "omo"), (false, "agents")] {
        let home = home_with(&[".omo", ".agents"]);
        let home = home.path();
        alias_omo_to_agents(home);
        for round in 0..2 {
            let writes = if explicit {
                corral_ops::install_harness_skills(home, harness, false).unwrap()
            } else {
                corral_ops::ensure_skills(home, harness).unwrap()
            };
            assert!(
                writes.iter().all(|w| w.action != SkillAction::Removed),
                "{harness} round {round}"
            );
            assert_all_roles_present(home);
            assert!(corral_ops::duplicate_skills(home, "omo").is_empty());
        }
    }
}

#[test]
fn installing_into_the_agents_target_removes_owned_omo_copies() {
    for explicit in [true, false] {
        let home = home_with(&[".omo", ".agents"]);
        let home = home.path();
        for role in ROLE_SKILLS {
            put(
                &omo(home, role),
                corral_ops::role_skill_markdown(role).unwrap().as_bytes(),
            );
        }
        put(&omo(home, "mine"), b"not corral");
        assert_eq!(
            corral_ops::duplicate_skills(home, "omo").len(),
            ROLE_SKILLS.len()
        );

        let writes = if explicit {
            corral_ops::install_harness_skills(home, "agents", false).unwrap()
        } else {
            corral_ops::ensure_skills(home, "agents").unwrap()
        };
        let removed = writes.iter().filter(|w| w.action == SkillAction::Removed);
        assert_eq!(removed.count(), ROLE_SKILLS.len(), "explicit={explicit}");
        for role in ROLE_SKILLS {
            assert!(!omo(home, role).exists(), "{role}");
        }
        assert_eq!(std::fs::read(omo(home, "mine")).unwrap(), b"not corral");
        assert!(corral_ops::duplicate_skills(home, "omo").is_empty());
        assert_all_roles_present(home);
    }
}

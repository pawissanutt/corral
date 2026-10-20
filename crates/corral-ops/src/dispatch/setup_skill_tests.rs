use corral_core::{Status, Store};

use crate::Ctx;
use crate::skill::{PLANNER_SKILL, SKILL_MARKER, WORKER_SKILL, role_skill_markdown};

fn skill_events(ctx: &Ctx, task: &str) -> Vec<(String, String)> {
    ctx.store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(task) && e.kind.starts_with("skill_"))
        .map(|e| {
            (
                e.kind,
                e.detail["role"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn dispatch_refreshes_stale_managed_skills_and_keeps_unmanaged_files() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(&tmp.path().join("ws")).unwrap();
    let task = store.create("Skill", None, Status::Ready, "Spec").unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let home = tmp.path().join("home");
    let skills = home.join(".claude/skills");
    std::fs::create_dir_all(skills.join(WORKER_SKILL)).unwrap();
    std::fs::create_dir_all(skills.join(PLANNER_SKILL)).unwrap();
    let stale = skills.join(WORKER_SKILL).join("SKILL.md");
    std::fs::write(&stale, format!("{SKILL_MARKER}\nold\n")).unwrap();
    let foreign = skills.join(PLANNER_SKILL).join("SKILL.md");
    std::fs::write(&foreign, "mine").unwrap();

    super::install_skill_in(&ctx, &task.id, "claude", &home).unwrap();

    assert_eq!(
        Some(std::fs::read_to_string(&stale).unwrap()),
        role_skill_markdown(WORKER_SKILL)
    );
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "mine");
    let first = skill_events(&ctx, &task.id);
    let has = |events: &[(String, String)], kind: &str, role: &str| {
        events.iter().any(|(k, r)| k == kind && r == role)
    };
    assert!(has(&first, "skill_refreshed", WORKER_SKILL), "{first:?}");
    assert!(has(&first, "skill_unmanaged", PLANNER_SKILL), "{first:?}");
    assert!(has(&first, "skill_installed", "corral"), "{first:?}");

    // A second dispatch finds every managed copy current and writes nothing.
    super::install_skill_in(&ctx, &task.id, "claude", &home).unwrap();
    let second = skill_events(&ctx, &task.id);
    assert_eq!(
        second[first.len()..],
        [("skill_unmanaged".to_string(), PLANNER_SKILL.to_string())]
    );
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "mine");
}

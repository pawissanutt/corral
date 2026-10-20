//! The plan files the planner skill shows must pass the real `plan_submit` validation.
use corral_core::{Status, Store};
use corral_ops::{PLANNER_SKILL, plan_submit, role_skill_markdown};

/// Every fenced `markdown` block in the skill that is a complete plan file.
fn example_plans(skill: &str) -> Vec<String> {
    let mut plans = Vec::new();
    let mut current: Option<String> = None;
    for line in skill.lines() {
        match (&mut current, line) {
            (None, "```markdown") => current = Some(String::new()),
            (Some(_), "```") => plans.extend(current.take()),
            (Some(block), line) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, _) => {}
        }
    }
    plans.retain(|plan| plan.starts_with("---\n"));
    plans
}

#[test]
fn planner_skill_examples_pass_plan_submit() {
    let skill = role_skill_markdown(PLANNER_SKILL).unwrap();
    let examples = example_plans(&skill);
    assert_eq!(examples.len(), 2, "single-task and multi-task examples");

    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Spin widget", None, Status::Ready, "Spin clockwise.")
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);

    let mut records = Vec::new();
    for (index, example) in examples.iter().enumerate() {
        let path = tmp.path().join(format!("plan-{index}.md"));
        std::fs::write(&path, example).unwrap();
        records.push(plan_submit(&ctx, &task.id, &path).unwrap());
    }
    assert!(records[0].document.dependencies.is_empty());
    let scopes = &records[1].document.dependencies;
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[1].depends_on, ["widget-api"]);
}

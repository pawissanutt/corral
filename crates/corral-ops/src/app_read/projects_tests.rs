use super::*;
use corral_core::Store;
use serde_json::json;

#[test]
fn configured_projects_count_their_tasks_and_plans_name_member_projects() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let mut ctx = Ctx::from_store(Store::init(dir.path())?);
    ctx.store.config.projects.clear();
    for (id, path) in [
        ("alpha", ctx.store.root().join("alpha")),
        ("beta", outside.path().to_path_buf()),
    ] {
        std::fs::create_dir_all(&path)?;
        ctx.store
            .config
            .projects
            .insert(id.into(), serde_json::from_value(json!({"path": path}))?);
    }
    ctx.store.save_config()?;
    let alpha = ctx
        .store
        .create("Alpha work", Some("alpha"), Status::Running, "")?;
    ctx.store.update(&alpha.id, |task| {
        task.runs
            .push(crate::agent::test_support::run("R1", "project-worker"));
        Ok(())
    })?;
    let beta = ctx
        .store
        .create("Beta work", Some("beta"), Status::Review, "")?;
    let unassigned = ctx.store.create("Unassigned", None, Status::Backlog, "")?;
    let plan = ctx.store.create("Design", None, Status::Backlog, "")?;
    ctx.store.update(&plan.id, |task| {
        task.design = true;
        task.children = vec![beta.id.clone(), unassigned.id.clone(), alpha.id.clone()];
        Ok(())
    })?;

    let rows = super::super::projects_rows(&ctx)?;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].id.as_str(), rows[0].path.as_str()),
        ("alpha", "alpha")
    );
    assert_eq!(rows[1].id, "beta");
    assert_eq!(rows[1].path, outside.path().display().to_string());
    assert_eq!((rows[0].open, rows[0].running, rows[0].review), (1, 1, 0));
    assert_eq!((rows[1].open, rows[1].running, rows[1].review), (1, 0, 1));
    assert!(rows.iter().all(|row| !row.in_pm_scope && !row.autopilot));
    assert_eq!(super::super::projects_rows(&ctx)?, rows);
    let plans = super::super::plans_rows(&ctx)?;
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].projects, ["alpha", "beta"]);

    ctx.store.update(&alpha.id, |task| {
        task.status = Status::Dropped;
        Ok(())
    })?;
    let changed = super::super::projects_rows(&ctx)?;
    assert_eq!(changed[0].open, 0);
    assert_ne!(changed[0].rev, rows[0].rev);
    assert_eq!(changed[1].rev, rows[1].rev);
    ctx.store.config.projects.clear();
    ctx.store.save_config()?;
    assert!(super::super::projects_rows(&ctx)?.is_empty());
    Ok(())
}

#[test]
fn reverted_task_with_completed_run_is_not_in_review() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut ctx = Ctx::from_store(Store::init(dir.path())?);
    ctx.store.config.projects.clear();
    ctx.store.config.projects.insert(
        "alpha".into(),
        serde_json::from_value(json!({"path": dir.path()}))?,
    );
    ctx.store.save_config()?;
    let task = ctx
        .store
        .create("Reverted", Some("alpha"), Status::Backlog, "")?;
    ctx.store.update(&task.id, |task| {
        task.dispatch_hold = true;
        let mut run = crate::agent::test_support::run("R1", "project-worker");
        run.outcome = Some(RunOutcome::Done);
        task.runs.push(run);
        Ok(())
    })?;

    assert_eq!(super::super::projects_rows(&ctx)?[0].review, 0);
    Ok(())
}

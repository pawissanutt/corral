use anyhow::Result;
use corral_ops::Ctx;

pub(super) fn print(ctx: &Ctx) -> Result<()> {
    let config = &ctx.store.config;
    println!("{}", corral_ops::automation_summary(ctx)?);
    let usage = corral_ops::slot_usage(ctx)?;
    let slots = corral_tui::board::slot_summary(&usage, &config.limits);
    println!("slots: {slots}");
    println!(
        "dispatch: paused={}, auto_backlog={}, promote_ideas={:?}, daily cap={}",
        config.dispatch.paused,
        config.dispatch.auto_backlog,
        config.dispatch.promote_ideas,
        config.dispatch.max_auto_promotions_per_day
    );
    for project in config.projects.keys() {
        let policy = config.dispatch_policy(Some(project));
        println!(
            "  {project}: auto_backlog={}, promote_ideas={:?}, daily cap={}",
            policy.auto_backlog, policy.promote_ideas, policy.max_auto_promotions_per_day
        );
    }
    let automatic = corral_ops::automatic_dispatch_status(ctx)?;
    println!(
        "automatic coding slots: {}; promotions today (UTC): {}",
        automatic.free_slots, automatic.promotions_today
    );
    for event in automatic.recent {
        println!(
            "  policy {} {}: {}",
            event.ts,
            event.task.as_deref().unwrap_or("-"),
            event.detail
        );
    }
    Ok(())
}

use anyhow::Result;
use corral_ops::Ctx;

use super::route;

pub(super) fn run(ctx: &Ctx, id: &str, text: &[String], resend: bool) -> Result<()> {
    let task = if resend {
        corral_ops::resend_reply(ctx, id)?
    } else {
        corral_ops::reply(ctx, id, &text.join(" "))?
    };
    if task
        .current_run()
        .is_some_and(|run| run.pending_reply.is_some())
    {
        println!(
            "{} reply not delivered; pending (use reply {} --resend)",
            task.id, task.id
        );
    } else {
        println!("{} replied ({})", task.id, task.status.label());
    }
    Ok(())
}

pub(super) fn changes(ctx: &Ctx, id: &str, text: &[String], escalate: bool) -> Result<()> {
    if escalate {
        route::print_outcome(corral_ops::request_changes_escalated(
            ctx,
            id,
            &text.join(" "),
        )?);
    } else {
        let task = corral_ops::request_changes(ctx, id, &text.join(" "))?;
        println!("{} -> {}", task.id, task.status.label());
    }
    Ok(())
}

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::Target;
use crate::Ctx;

pub(crate) fn relaunch(ctx: &Ctx, target: &Target, replace: bool) -> Result<Value> {
    target.require_task_family()?;
    if target.role == "pm" {
        ensure!(
            target.owned,
            "not_owned: PM pane belongs to another herdr server"
        );
        // A synthetic ended PM row is the app's Launch PM control.
        if target.ended || !target.bridge {
            let detail = pm_open(ctx)?;
            super::pause::clear_generation(ctx, target)?;
            return Ok(detail);
        }
        let pane = crate::pm::relaunch_pm(ctx, "owner_session_relaunch", replace)
            .context("relaunching PM session")?;
        super::pause::clear_generation(ctx, target)?;
        return Ok(json!({"id": "pm", "pane": pane, "relaunched": true}));
    }
    ensure!(!target.ended, "ended: session {} has ended", target.id);
    if target.pane.is_some() {
        ensure!(
            target.owned,
            "not_owned: session pane belongs to another herdr server"
        );
        ensure!(
            !target.adopted,
            "adopted_pane: cannot launch into the owner's pane"
        );
    }
    let container = target
        .container
        .as_ref()
        .context("no_bridge: session has no recoverable host container")?;
    // Explicit owner relaunch releases only this generation's session fence before
    // paid-start admission; it never releases an availability/financial pause.
    super::pause::clear_generation(ctx, target)?;
    let detail = crate::agent::recovery::resume_container(ctx, container, replace)
        .context("relaunching session container")?;
    Ok(detail)
}

pub(crate) fn pm_open(ctx: &Ctx) -> Result<Value> {
    ensure!(
        crate::session_scope::owns_inventory(ctx)?,
        "not_owned: workspace panes belong to another herdr server"
    );
    let pane = crate::pm::open_pm_unfocused(ctx).context("opening PM session")?;
    Ok(json!({"id": "pm", "pane": pane}))
}

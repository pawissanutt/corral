//! Pane excerpt capture and launch-refusal guidance shared by sync transitions.
use crate::Ctx;

pub(super) fn read_excerpt(ctx: &Ctx, pane_id: &str) -> String {
    match ctx.herdr.pane_read(pane_id, 80) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            lines[lines.len().saturating_sub(12)..].join("\n")
        }
        Err(err) => format!("(could not read agent pane: {err:#})"),
    }
}

pub(super) fn launch_refusal_hint(text: String) -> String {
    if text.contains("corral agent launch refused: memory_contract_unavailable:") {
        format!(
            "{text}\nFix: run `corral agent install` or update the configured Pi assets, then retry."
        )
    } else {
        text
    }
}

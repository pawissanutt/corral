//! The header and editor project the same read-only workset as CLI status and PM brief.
use corral_ops::{Ctx, PmAction, PmWorkset};

pub(crate) fn reason(action: &PmAction) -> String {
    corral_ops::pm_read::action_reason(action)
}

pub(crate) fn readable_question(text: &str) -> String {
    corral_ops::pm_read::readable_question(text)
}

pub(crate) fn summary(workset: &PmWorkset) -> String {
    corral_ops::pm_read::operating_summary(workset)
}

pub(crate) fn readback(ctx: &Ctx) -> String {
    readback_result(corral_ops::pm_workset(ctx))
}

pub(super) fn readback_result(result: anyhow::Result<PmWorkset>) -> String {
    corral_ops::pm_read::readback_result(result)
}

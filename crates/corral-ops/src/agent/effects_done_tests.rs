//! Completion's real native-request entry is head/keyed-only, including audited replay.
use super::*;

pub(super) fn call(ctx: &Ctx, origin: &RequestOrigin) -> anyhow::Result<Receipted> {
    let report = Report {
        origin,
        gate_ref: None,
        contract_sha256: Some("c0".into()),
    };
    let _hot = corral_core::work::HotScope::enter();
    let before = corral_core::work::snapshot();
    let result = crate::review::done_request(
        ctx,
        origin.identity.task.as_deref().unwrap(),
        SUMMARY,
        None,
        &report,
    );
    let delta = corral_core::work::snapshot().since(&before);
    assert_eq!(
        delta.total(corral_core::work::Counter::FullTaskHydrations),
        0,
        "{:?}",
        delta.sites
    );
    result
}

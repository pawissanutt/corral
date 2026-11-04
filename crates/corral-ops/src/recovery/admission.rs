use super::*;
use std::cell::RefCell;

thread_local! { static ACTIVE: RefCell<Option<RecoveryRecord>> = const { RefCell::new(None) }; }

pub(super) fn with_admission<T>(
    record: &RecoveryRecord,
    apply: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with(|a| {
                a.replace(None);
            });
        }
    }
    ensure!(
        ACTIVE.with(|a| a.borrow().is_none()),
        "nested recovery admission is forbidden"
    );
    ACTIVE.with(|a| {
        a.replace(Some(record.clone()));
    });
    let _reset = Reset;
    apply()
}

pub(crate) fn admission_attempt(
    ctx: &Ctx,
    id: &str,
    stage: RecoveryAdmissionStage,
) -> Result<Option<String>> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(record) = active.as_ref() else {
            return Ok(None);
        };
        ensure!(
            record.preview.request.target.task == id,
            "resource recovery target changed"
        );
        check_resource(ctx, id, record, stage)?;
        let RecoveryEffect::Admission { attempt, .. } = &record.preview.proposed_effect else {
            anyhow::bail!("invalid admission recovery")
        };
        Ok(Some(attempt.clone()))
    })
}

pub(crate) fn bind_active_journal(ctx: &Ctx, authorization: &ActionAuthorization) -> Result<()> {
    ACTIVE.with(|active| match active.borrow().as_ref() {
        Some(record) => bind_journal(ctx, record, authorization),
        None => Ok(()),
    })
}

pub(crate) fn active_journal(launch: &crate::review_budget::ReviewLaunch<'_>) -> Result<()> {
    ACTIVE.with(|active| match active.borrow().as_ref() {
        Some(record) => launch.recovery_journal(record),
        None => Ok(()),
    })
}

pub(crate) fn require_no_pending(
    ctx: &Ctx,
    id: &str,
    supplied: Option<&RecoveryRecord>,
) -> Result<()> {
    let active = ACTIVE.with(|a| a.borrow().as_ref().map(|r| r.id.clone()));
    for record in records(ctx, id)? {
        if matches!(
            state(&record)?,
            RecoveryState::Authorized | RecoveryState::Applying | RecoveryState::Unknown
        ) {
            ensure!(
                supplied.is_some_and(|r| r.id == record.id) || active.as_ref() == Some(&record.id),
                "recovery pending or uncertain; reconcile the authorized operation, never relaunch it"
            );
        }
    }
    Ok(())
}

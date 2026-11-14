//! App admission is deliberately independent of CLI routing. New verbs default to refused.
use corral_protocol::{
    app::ActClass,
    cmd::{Command, config::ConfigCommand as Config},
};

/// Shared by preview and execution: the protocol's static class, minus settings keys the app
/// may never write. Confirmation/registry freshness remain daemon responsibilities.
pub fn app_class(command: &Command) -> ActClass {
    match command {
        Command::Config(Config::Set { key, .. }) if !super::config::app_key(key) => {
            ActClass::Refused
        }
        _ => command.app_class(),
    }
}

pub(super) fn admit(invoker: &super::Invoker, command: &Command) -> anyhow::Result<()> {
    if !invoker.is_app() {
        return Ok(());
    }
    let device = invoker.device()?;
    match app_class(command) {
        ActClass::Reply => {
            anyhow::ensure!(device.reply, "authority_reply: reply authority required")
        }
        ActClass::Act | ActClass::Confirm => {
            anyhow::ensure!(device.act, "authority_act: action authority required");
        }
        ActClass::Admin => {
            anyhow::ensure!(
                invoker.device_admin()?,
                "admin_only: trusted native administrator required"
            );
        }
        _ => anyhow::bail!("not_app_safe: command is not available to app devices"),
    }
    Ok(())
}

/// Preserve canonical refusal detail while distinguishing stale app views from failed effects.
pub(super) fn app_error(error: anyhow::Error) -> anyhow::Error {
    let question_conflict = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<crate::questions::QuestionRefusal>())
        .is_some_and(|refusal| {
            matches!(
                refusal.kind,
                crate::questions::RefusalKind::StaleRevision
                    | crate::questions::RefusalKind::State
                    | crate::questions::RefusalKind::Closed
                    | crate::questions::RefusalKind::Withdrawn
            )
        });
    let annotation_conflict = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<corral_core::annotations::AnnotationError>())
        .is_some_and(|refusal| {
            matches!(
                refusal.code,
                corral_core::annotations::AnnotationCode::StaleCandidate
                    | corral_core::annotations::AnnotationCode::CandidateNotCurrent
                    | corral_core::annotations::AnnotationCode::ThreadRevisionConflict
                    | corral_core::annotations::AnnotationCode::InvalidTransition
            )
        });
    if question_conflict || annotation_conflict {
        let message = error.to_string();
        error.context(corral_core::backend::ApplyError::Precondition(message))
    } else {
        error
    }
}

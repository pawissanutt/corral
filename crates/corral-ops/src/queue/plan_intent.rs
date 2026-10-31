//! Current explicit planning intent, shared by queue and dispatch admission.
use corral_core::TaskRef;

pub(crate) fn queued_plan_requested<T: TaskRef + ?Sized>(task: &T) -> bool {
    *task.queued()
        && task
            .dispatch_plan()
            .as_ref()
            .is_some_and(|plan| plan.explicit_plan)
}

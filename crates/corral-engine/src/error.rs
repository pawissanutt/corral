pub use corral_types::{Next, Refusal};

pub fn refuse(reason: impl Into<String>, next: Vec<Next>) -> anyhow::Error {
    Refusal {
        reason: reason.into(),
        next,
    }
    .into()
}

pub fn as_refusal(error: &anyhow::Error) -> Option<&Refusal> {
    error.downcast_ref::<Refusal>()
}

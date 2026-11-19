//! The model pill, effort control and picker of a session's composer (Ask's included), over the
//! host's model catalog (`session`).

mod session;

pub(crate) use session::{session_picker, session_pill};

/// The disclosure triangle after the model's name: drawn chrome, not wording.
pub(crate) const DISCLOSURE: &str = " ▾";

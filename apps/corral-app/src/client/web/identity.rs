//! Storage, not the optional UI marker, decides whether an identity may resume.
pub(super) enum Stored<T> {
    Missing,
    Credential(T),
    ForgetPending,
}

impl<T> Stored<T> {
    pub(super) fn for_startup(self, explicit_pairing: bool) -> Self {
        match self {
            Self::Credential(_) if explicit_pairing => Self::Missing,
            state => state,
        }
    }
}

#[derive(Default)]
pub(super) enum ForgetState {
    #[default]
    Idle,
    Pending,
}

impl ForgetState {
    pub(super) fn pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    pub(super) fn begin(&mut self) {
        *self = Self::Pending;
    }

    pub(super) fn complete(&mut self, result: Result<(), String>) -> Result<(), String> {
        result?;
        *self = Self::Idle;
        Ok(())
    }
}

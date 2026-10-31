//! The public approval choice, shared by landing and recorded cleanup authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveStrategy {
    FastForward,
    Rebase,
    KeepBranch,
}

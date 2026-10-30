//! Recovery status labels, independent of a container's accumulated state.
use corral_core::agent::HostHealth;

pub(super) fn health(health: HostHealth) -> &'static str {
    match health {
        HostHealth::Unbound => "unbound",
        HostHealth::Healthy => "healthy",
        HostHealth::Lost => "lost",
        HostHealth::Fenced => "fenced",
    }
}

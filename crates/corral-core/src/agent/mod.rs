//! Pi-native agent host records (T-252): versioned shapes stored through the Store API, the
//! pure liveness and fence predicate shared by every final transaction, and the container
//! closure predicate. Every shape, bound and storage class is listed in
//! docs/records/pi-native-records.md for T-253, which owns their schema and migration.

mod live;
pub mod paths;
pub mod pm_state;
mod question;
mod records;
#[cfg(test)]
mod tests;

pub use live::*;
pub use pm_state::AgentPm;
pub use question::*;
pub use records::*;

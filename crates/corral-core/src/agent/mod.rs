//! Pi-native agent host records (T-252): versioned shapes stored through the Store API, the
//! pure liveness and fence predicate shared by every final transaction, and the container
//! closure predicate. Every shape, bound and storage class is listed in
//! docs/records/pi-native-records.md for T-253, which owns their schema and migration.

mod capabilities;
mod jobs;
mod live;
mod local_controls;
pub mod paths;
pub mod pm_state;
mod question;
mod records;
pub mod rejections;
#[cfg(test)]
mod tests;
mod todo;

pub use capabilities::*;
pub use jobs::*;
pub use live::*;
pub use local_controls::*;
pub use pm_state::AgentPm;
pub use question::*;
pub use records::*;
pub use todo::*;

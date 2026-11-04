//! corral-pi agent host records (T-252): versioned shapes stored through the Store API, the
//! pure liveness and fence predicate shared by every final transaction, and the container
//! closure predicate. Every shape, bound and storage class is listed in
//! docs/records/pi-native-records.md for T-253, which owns their schema and migration.

mod auxiliary;
mod browser;
mod budget;
mod capabilities;
mod jobs;
mod live;
mod local_controls;
mod lsp;
pub(crate) mod native_inputs;
mod native_record;
pub mod paths;
mod pm_records;
pub mod pm_state;
mod question;
mod records;
pub mod rejections;
pub mod request;
mod rewind;
#[cfg(test)]
mod tests;
mod todo;
mod usage;
mod web;
pub mod write_permissions;

pub use auxiliary::{ExploreSettings, OracleSettings};
pub use browser::canonical_origin;
pub use budget::*;
pub use capabilities::*;
pub use jobs::*;
pub use live::*;
pub use local_controls::*;
pub use native_record::NativeRecord;
pub use pm_state::AgentPm;
pub use question::*;
pub use records::*;
pub use rewind::*;
pub use todo::*;
pub use usage::*;
pub use web::{WebSearchProvider, WebSettings};

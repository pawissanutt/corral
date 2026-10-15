//! corral-core: task model, `.corral/` file store, config, events log.
//! Design: docs/DESIGN.md sections 4 and 5.

pub mod config;
pub mod model;
pub mod profile;
pub mod store;

pub use config::*;
pub use model::*;
pub use profile::*;
pub use store::*;

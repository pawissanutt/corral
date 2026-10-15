//! corral-core: task model, `.corral/` file store, config, events log.
//! Design: docs/DESIGN.md sections 4 and 5.

pub mod completion;
pub mod config;
pub mod config_doc;
pub mod event_reader;
pub mod hints;
pub mod messages;
pub mod model;
pub mod obligations;
pub mod pins;
pub mod plan;
pub mod pricing;
pub mod profile;
pub mod review;
pub mod routing;
pub mod store;
mod usage;
mod verification;

pub use completion::*;
pub use config::*;
pub use config_doc::ConfigDoc;
pub use event_reader::{EventCursor, EventReader, actionable_kind, format_event_line};
pub use hints::*;
pub use messages::{Message, MessageGuard, capture_messages, report as report_message};
pub use model::*;
pub use obligations::*;
pub use pins::*;
pub use plan::*;
pub use pricing::Price;
pub use profile::*;
pub use review::*;
pub use routing::*;
pub use store::*;
pub use verification::*;

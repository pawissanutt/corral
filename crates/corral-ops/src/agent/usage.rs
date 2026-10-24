//! Pi-native usage from the run's bound Pi session files, priced at API-equivalent rates and
//! deduplicated by (session ID, response ID). Owner: T-252 child pi-usage.
use anyhow::{Result, bail};
use corral_core::{Run, Store, Usage};

/// The T-252 child that lands the collector.
pub const OWNER: &str = "pi-usage";

/// The note a pi-native run carries until pi-usage lands.
pub const NOT_INSTALLED: &str = "pi-native usage collector not installed";

pub fn collect(_run: &Run, _store: &Store) -> Result<Usage> {
    bail!(NOT_INSTALLED)
}

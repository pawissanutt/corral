//! Usage seam: `usage::collect` routes pi-native runs and review children here.
//! Owner: T-252 child pi-usage.
use anyhow::Result;
use corral_core::{Run, Store, Usage};

use super::consulted;

pub fn collect(run: &Run, store: &Store) -> Result<Usage> {
    consulted("usage");
    super::super::usage::collect(run, store)
}

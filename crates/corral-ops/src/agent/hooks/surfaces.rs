//! Instruction-surface seam: the Pi contract assets live in `PI_ASSETS`, and the T-251 drift
//! test (crates/corral-ops/tests/instruction_surfaces.rs) scans every Markdown file there, so a
//! contract asset added later is covered without editing the test.
//! Owner: T-252 child worker-role (renders the contract prompts from these assets).

/// The Pi contract asset directory, relative to the repository root.
pub const PI_ASSETS: &str = "packages/corral-pi/assets";

//! Sessions: every session of every role. The session page itself is `screens::agents::session`.
//!
//! This is the first cut of Sessions: the former Agents roster, with its New menu, role tabs
//! and Show switch, under the one project scope. The list grouped by state beside an inspector
//! (`model::sessions`, `ui::kit::list`) replaces it here.

use nsh_core::prelude::*;

use crate::screens::agents;
use crate::state::AppState;

pub fn screen(app: AppState) -> AnyElement {
    agents::screen(app)
}

//! Activity › Host performance and Home's Host performance widget (`model::host`).

use nsh_core::prelude::*;

use crate::state::AppState;

/// Activity › Host performance.
pub fn screen(app: AppState) -> AnyElement {
    let _ = app;
    view().into_element()
}

/// Home's Host performance widget.
pub fn widget(app: AppState) -> AnyElement {
    let _ = app;
    view().into_element()
}

use nsh_core::prelude::*;

use crate::state::AppState;

pub fn screen(_app: AppState) -> AnyElement {
    view().into_element()
}

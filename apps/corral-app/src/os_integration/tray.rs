//! The tray icon: the way back to Corral once its window is closed to the background, and the
//! one place to quit it from then.

use nsh_core::element::desktop::{MenuItem, main_window, quit, set_tray, tray};
use nsh_core::prelude::*;

use crate::state::AppState;

/// The brand mark: square and single-colour, so macOS draws it as a menu-bar template.
const ICON: ImageAsset = asset!("icons/brand.svg");

/// Shows the tray icon: a click brings the main window back; its menu shows the window, opens
/// Ask in it, or quits.
pub(super) fn install(app: AppState) {
    set_tray(
        tray()
            .icon(ICON)
            .tooltip("Corral")
            .menu([
                MenuItem::new("Show Corral").on_select(raise),
                MenuItem::new("Ask").on_select(move || {
                    raise();
                    app.open_ask();
                }),
                MenuItem::separator(),
                MenuItem::new("Quit Corral").on_select(quit),
            ])
            .on_click(|_| raise()),
    );
}

/// Shows the main window if it was hidden and gives it the focus.
fn raise() {
    let window = main_window();
    window.show();
    window.focus();
}

//! Native desktop entry points. The shortcut raises the real Ask composer, not the fixture
//! hotkey illustration. Closing the main window hides it so its root and shortcut stay alive;
//! Quit in the tray ends the process. NSH releases root-owned registrations on unmount.
//!
//! `CORRAL_ASK_SHORTCUT` overrides `mod+shift+space` (NSH keystroke syntax). Linux may ask the
//! owner to choose a binding through the desktop portal; no global keys are claimed on web/TUI.

mod tray;

use nsh_core::desktop::{Shortcut, WindowAttributes};
use nsh_core::element::desktop::{main_window, register_global_shortcut};
use nsh_core::platform::Os;
use nsh_core::prelude::*;

use crate::state::AppState;

const DEFAULT_SHORTCUT: &str = "mod+shift+space";

/// Installed once under the app root, never under the drawer's shorter-lived owner.
pub(crate) fn install(app: AppState) {
    if !os().is_desktop() {
        return;
    }
    tray::install(app);

    let main = main_window();
    let mut attributes = WindowAttributes::default();
    attributes.close_on_press = Some(false);
    main.set(attributes);
    let closing = main.clone();
    main.on_close_press(move |_| closing.hide());

    let shortcut = match std::env::var("CORRAL_ASK_SHORTCUT") {
        Ok(shortcut) => shortcut,
        Err(std::env::VarError::NotPresent) => DEFAULT_SHORTCUT.to_owned(),
        Err(error) => {
            app.toast(format!("Ask global shortcut was not registered: {error}"));
            return;
        }
    };
    // NSH's registration panics on invalid syntax; an owner override must fail visibly instead.
    if let Err(error) = Shortcut::parse(&shortcut, os() == Os::MacOs) {
        app.toast(format!("Ask global shortcut was not registered: {error}"));
        return;
    }
    let registered = register_global_shortcut(&shortcut, move || {
        main.focus();
        app.open_ask();
    });
    spawn(async move {
        if !registered.await {
            app.toast(format!(
                "Ask global shortcut {shortcut} is unavailable or already in use. Use Ask in Corral or set CORRAL_ASK_SHORTCUT and restart."
            ));
        }
    });
}

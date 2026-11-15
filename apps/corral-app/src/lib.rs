//! Corral's app shell: the navigation frame on desktop and web, phone and terminal. `nsh run`,
//! `nsh package` and `cargo run` mount [`App`].

pub mod client;
pub mod model;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod os_integration;
pub mod screens;
pub mod shell;
pub mod state;
pub mod ui;

use nsh_core::element::Idiom;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::tokens::{Token, color};

/// The app's UI set (`nsh.json` `"rust"`): its root component and the files of `assets/`, whose
/// fonts the engine registers before the first render.
#[native_module("corral/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
    registry.assets(assets!());
}

/// A tablet whose longer edge reaches this gets the desktop shell, whose sidebar folds to its
/// rail while the window is narrow; a smaller one keeps the phone's tabs. The longer edge, so
/// turning the tablet does not change the shell it started with.
const PAD_DESKTOP: f32 = 900.0;

/// The root: the shell of the device's idiom over the app's state.
#[component]
pub fn App() -> impl IntoElement {
    let form = match idiom() {
        Idiom::Terminal => Form::Terminal,
        Idiom::Desktop => Form::Desktop,
        Idiom::Pad if long_edge() >= PAD_DESKTOP => Form::Desktop,
        _ => Form::Phone,
    };
    let app = AppState::install(form);
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    if form == Form::Desktop {
        os_integration::install(app);
    }
    let frame = match form {
        Form::Desktop => shell::desktop::shell(app),
        Form::Phone => shell::phone::shell(app),
        Form::Terminal => shell::terminal::shell(app),
    };
    view()
        .flex_1()
        .element_ref(&app.focus_root)
        .bg(color(Token::Bg))
        .text_color(color(Token::Ink))
        .child(frame)
}

/// The window's longer edge, read once: the shell is chosen when the app starts.
fn long_edge() -> f32 {
    let size = untracked(window);
    size.width.0.max(size.height.0)
}

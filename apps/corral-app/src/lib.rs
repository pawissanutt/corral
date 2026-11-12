//! Corral's app shell: the navigation frame on desktop and web, phone and terminal. `nsh run`,
//! `nsh package` and `cargo run` mount [`App`].

pub mod client;
pub mod model;
pub mod screens;
pub mod shell;
pub mod state;
pub mod ui;

use nsh_core::element::Idiom;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::tokens::{Token, color};

/// The app's UI set (`nsh.json` `"rust"`): its root component and the files of `assets/`, whose two
/// fonts the engine registers before the first render.
#[native_module("corral/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
    registry.assets(assets!());
}

/// The root: the shell of the device's idiom over the app's state.
#[component]
pub fn App() -> impl IntoElement {
    let form = match idiom() {
        Idiom::Terminal => Form::Terminal,
        Idiom::Desktop => Form::Desktop,
        _ => Form::Phone,
    };
    let app = AppState::install(form);
    let frame = match form {
        Form::Desktop => shell::desktop::shell(app),
        Form::Phone => shell::phone::shell(app),
        Form::Terminal => shell::terminal::shell(app),
    };
    view()
        .flex_1()
        .bg(color(Token::Bg))
        .text_color(color(Token::Ink))
        .child(frame)
}

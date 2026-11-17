//! Pure assistant interaction state and feed-backed content for every form.

mod actions;
mod attach;
mod billing;
mod cards;
mod labels;
mod live;
mod live_act;
mod live_cards;
mod live_chips;
mod live_filings;
mod live_parts;
mod live_thread;
#[cfg(test)]
mod parts_tests;
mod project;
#[cfg(test)]
mod question_tests;
mod surfaces;
mod thread;
mod views;

pub use actions::*;
pub use attach::*;
pub use cards::*;
pub use labels::*;
pub use live::*;
pub use live_act::*;
pub use live_cards::*;
pub use live_filings::*;
pub use live_thread::*;
pub use project::*;
pub use surfaces::*;
pub use thread::*;
pub use views::*;

use crate::model::effect::Effect;

/// Device-local assistant interaction state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    pub model_menu: bool,
    pub anchor: Option<(ThreadRef, String)>,
    pub live: LiveState,
}

impl State {
    /// Retire the one-shot Inbox reveal.
    pub fn clear_anchor(&mut self) {
        self.anchor = None;
    }

    /// Close the model picker before closing Ask.
    pub fn close_menu(&mut self) -> bool {
        let open = self.model_menu;
        self.model_menu = false;
        open
    }

    pub fn close_ask(&mut self) -> Vec<Effect> {
        self.model_menu = false;
        vec![Effect::CloseAsk]
    }
}

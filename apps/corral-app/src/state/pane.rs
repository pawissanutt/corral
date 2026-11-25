//! The desktop Ask pane's preference and the room beside the sidebar.

use nsh_core::prelude::*;

use crate::model::pane::{PANE_DEFAULT, Placement, kept, placement};

use super::{preference, remember};

const KEY: &str = "corral.pane";

#[derive(Clone, Copy)]
pub struct Pane {
    width: Signal<f32>,
    room: Signal<f32>,
    pub focused: Signal<bool>,
}

impl Pane {
    pub(super) fn new() -> Self {
        let width = preference(KEY, |value| {
            value.parse::<f32>().ok().filter(|w| w.is_finite())
        });
        Self {
            width: signal(kept(width.unwrap_or(PANE_DEFAULT))),
            room: signal(0.0),
            focused: signal(false),
        }
    }

    pub fn placement(self) -> Placement {
        placement(self.room.get(), self.width.get())
    }

    pub fn set_room(self, room: f32) {
        if self.room.get_untracked() != room {
            self.room.set(room);
        }
    }

    pub fn resize(self, width: f32) {
        let width = kept(width);
        self.width.set(width);
        remember(KEY, &width.to_string());
    }
}

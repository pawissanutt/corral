//! Home's board on this device (`model::home::board`): the arrangement, saved under
//! `corral.home.board` on every change, and whether Customize is on.

use nsh_core::prelude::*;

use crate::model::home::board::{Board, KEY};

use super::{preference, remember};

#[derive(Clone, Copy)]
pub struct HomeBoard {
    pub board: Signal<Board>,
    /// Customize is on: widgets show their handles and Add widget shows.
    pub editing: Signal<bool>,
}

impl HomeBoard {
    pub(super) fn new() -> Self {
        Self {
            board: signal(preference(KEY, Board::parse).unwrap_or_default()),
            editing: signal(false),
        }
    }

    /// Changes the arrangement and keeps it.
    pub fn change(self, change: impl FnOnce(&mut Board)) {
        self.shape(change);
        self.save();
    }

    /// Changes the arrangement shown without keeping it yet: a resize drag between moves.
    pub fn shape(self, change: impl FnOnce(&mut Board)) {
        let mut next = self.board.get_untracked();
        change(&mut next);
        if self.board.with_untracked(|now| *now != next) {
            self.board.set(next);
        }
    }

    /// Keeps the arrangement shown.
    pub fn save(self) {
        remember(KEY, &self.board.with_untracked(Board::encode));
    }

    /// Back to the default arrangement.
    pub fn reset(self) {
        self.change(|board| *board = Board::default());
    }

    pub fn set_editing(self, on: bool) {
        if self.editing.get_untracked() != on {
            self.editing.set(on);
        }
    }
}

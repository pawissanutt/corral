//! Task detail's explicit handoff actions: `W` work on this here (take back) and `K` save
//! for later, through the SAME owner services as the board card and `/handoff`. The task's
//! contract revision as shown is bound; a changed task refuses as stale.
use corral_ops::Ctx;
use corral_ops::assistant::workflow::handoff::{owner, status};

use super::DetailView;
use crate::{Action, Toast, Tone, View};

impl DetailView {
    /// The handoff controls a transferred task offers, as keys and clickable hints.
    pub(super) fn handoff_hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.handoff.is_some() && status::transfer_fenced(&self.task) {
            true => vec![("W", "work on this here"), ("K", "save for later")],
            false => Vec::new(),
        }
    }

    pub(super) fn transfer_key(&mut self, ctx: &Ctx, take_back: bool) -> Action {
        if !status::transfer_fenced(&self.task) {
            return Action::Toast(Toast::new(
                Tone::Info,
                "No handoff on this task",
                "W/K apply only to work handed off from an assistant",
            ));
        }
        let id = self.task.id.clone();
        let outcome = corral_core::TaskRef::contract_revision(&self.task).and_then(|contract| {
            if take_back {
                owner::take_back(ctx, &id, &contract)
                    .map(|_| format!("{id} returning to assistant"))
            } else {
                owner::save(ctx, &id, &contract).map(|_| format!("{id} saved · Held"))
            }
        });
        self.refresh(ctx);
        match outcome {
            Ok(text) => Action::Toast(Toast::new(Tone::Info, text, "same service as the board")),
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                format!("{id}: nothing changed"),
                format!("{e:#}"),
            )),
        }
    }
}

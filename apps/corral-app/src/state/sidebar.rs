//! The desktop and terminal sidebar: the persisted wide choice, and the labels opened over the
//! content from the rail while the window or grid is too narrow to give them room.

use nsh_core::prelude::*;

use super::AppState;

impl AppState {
    /// Tracked: whether the narrow layout shows the sidebar's labels over the content.
    pub fn sidebar_peeking(&self) -> bool {
        self.sidebar_peek.get()
    }

    /// Wide: flips the persisted choice. Narrow: opens or closes the labels over the content
    /// for the moment, leaving the wide choice as it was for the return to wide.
    pub fn toggle_sidebar(&self) {
        let narrow = untracked(|| crate::shell::sidebar_narrow(*self));
        if narrow {
            self.sidebar_peek.update(|peek| *peek = !*peek);
        } else {
            self.sidebar_peek.set(false);
            self.sidebar.update(|pref| *pref = pref.toggled(false));
        }
    }

    /// Closes the labels shown over the content; whether they were open.
    pub fn close_sidebar_peek(&self) -> bool {
        let open = self.sidebar_peek.get_untracked();
        if open {
            self.sidebar_peek.set(false);
        }
        open
    }

    /// The labels over the content close once the layout turns wide (the sidebar has its room
    /// again), after any navigation they were opened for, and when Ask or the palette opens.
    pub(super) fn follow_sidebar(self) {
        let wide = computed(move || !crate::shell::sidebar_narrow(self));
        effect(move || {
            if wide.get() {
                untracked(|| self.close_sidebar_peek());
            }
        });
        let moved = computed(move || {
            self.nav.with(|nav| {
                (
                    nav.destination,
                    nav.goes,
                    nav.section_opens,
                    nav.pages.len(),
                    nav.overlays.len(),
                )
            })
        });
        effect(move || {
            moved.get();
            untracked(|| self.close_sidebar_peek());
        });
    }
}

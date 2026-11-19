//! The arrow keys through a menu the app draws itself rather than as an NSH popup (the
//! keyboard's Actions and Session menus, which open as a dialog): Up and Down move the keyboard
//! focus to the previous and next item, around the ends, Home and End to the first and last, as
//! NSH's own menus walk theirs. Return and Space press the focused item, and Tab still walks them.

use std::cell::Cell;
use std::rc::Rc;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::commands::{MENU_WALK, menu_step};

/// Binds the walk over `items`, in their order, while the current owner (the open menu) lives.
/// Follows the focus a press or Tab puts on an item, so the next arrow moves on from there.
pub fn walk(items: Vec<ElementRef>) {
    let at = Rc::new(Cell::new(0usize));
    for (index, item) in items.iter().enumerate() {
        let at = at.clone();
        item.on_focus_in(move || at.set(index));
    }
    let items = Rc::new(items);
    let options = KeyOptions {
        when: When::NotEditing,
        repeat: true,
        ..KeyOptions::default()
    };
    for key in MENU_WALK {
        let at = at.clone();
        let items = items.clone();
        on_key_with(key, options, move |_| {
            if let Some(next) = menu_step(key, at.get(), items.len())
                && let Some(item) = items.get(next)
            {
                at.set(next);
                item.focus();
            }
        });
    }
}

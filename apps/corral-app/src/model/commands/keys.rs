//! Which keys a place's list or open page answers, and the terminal's legend of them.
//!
//! List keys (moves, Enter, the inspector's `i`, More's `.`) are the keys a focused control uses
//! for itself: the shell binds them only while the list has the keyboard focus, or nothing has,
//! so Tab moves the focus, Enter and Space press the focused control and the arrows scroll
//! where no list wants them. The legend and the bindings read the same guards as the reducers.

use crate::model::{Destination, Live};

/// Home: move through what waits and what works, Enter opens the selected one's page.
const HOME_OPEN: &[&str] = &["j", "k", "down", "up", "enter", "."];
const HOME_MOVE: &[&str] = &["j", "k", "down", "up", "."];
/// Tasks: as Home, and `i` shows or hides the inspector beside the list.
const LIST_OPEN: &[&str] = &["j", "k", "down", "up", "enter", "i", "."];
const LIST_MOVE: &[&str] = &["j", "k", "down", "up", "i", "."];
/// Sessions: move, Enter opens the selected session, N (Shift+N) the New build sheet.
const SESSIONS_OPEN: &[&str] = &["j", "k", "down", "up", "enter", "N", "."];
const SESSIONS_MOVE: &[&str] = &["j", "k", "down", "up", "N", "."];
/// A task's or a session's page: `.` opens its More menu.
const MENU: &[&str] = &["."];

/// The list keys on `destination` under the page on top.
pub fn list_keys_live(destination: Destination, top: Option<&Live>) -> &'static [&'static str] {
    match (destination, top) {
        (_, Some(_)) => MENU,
        (Destination::Home, None) => HOME_OPEN,
        (Destination::Tasks, None) => LIST_OPEN,
        (Destination::Sessions, None) => SESSIONS_OPEN,
        _ => &[],
    }
}

/// `keys` as the keyboard focus leaves them: Enter opens the selected row's page only while
/// nothing has the focus or the selected row has it; on any other control of the list (a
/// fold, another row, a button beside it) Enter is that control's own press. `focus`: the
/// focused control, `None` when nothing has it, `Some(None)` for one that is not a row.
pub fn with_focus(
    keys: &'static [&'static str],
    focus: Option<Option<&str>>,
    selected: Option<&str>,
) -> &'static [&'static str] {
    match focus {
        Some(row) if row.is_none() || row != selected => {
            if keys == HOME_OPEN {
                HOME_MOVE
            } else if keys == LIST_OPEN {
                LIST_MOVE
            } else if keys == SESSIONS_OPEN {
                SESSIONS_MOVE
            } else {
                keys
            }
        }
        _ => keys,
    }
}

/// The terminal key hints of the place's list or of the page on top.
pub fn key_legend_live(
    destination: Destination,
    top: Option<&Live>,
) -> Vec<(&'static str, &'static str)> {
    match (destination, top) {
        (_, Some(Live::Session(_))) => vec![(".", "more")],
        (_, Some(_)) => vec![("Esc", "tasks"), (".", "more")],
        (Destination::Home, None) => vec![("j k", "move"), ("Enter", "open"), (".", "more")],
        (Destination::Tasks, None) => vec![
            ("j k", "move"),
            ("Enter", "open"),
            ("i", "inspector"),
            (".", "more"),
        ],
        (Destination::Sessions, None) => vec![
            ("j k", "move"),
            ("Enter", "open"),
            ("N", "build"),
            (".", "more"),
        ],
        _ => Vec::new(),
    }
}

/// What stands over the page and takes the keys while it is open: the page's own keys do
/// nothing then, so the terminal's key line gives the surface's instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The command palette.
    Palette,
    /// A list of choices: the More menu, the scope menu, the rewind sheet's turns.
    Menu,
    /// An operation's form (a comment, a dispatch's profile and base).
    Form,
    /// The refusal sheet's next steps.
    Sheet,
    /// The question asked once before a destructive or forced step, answered by typing its word.
    Confirm,
    /// An artifact shown full size in the finder's place.
    FullSize,
    /// The artifact finder's filter, which keeps the keyboard: letters type into it.
    Filter,
}

impl Surface {
    /// The surface's keys, in the key line's order.
    pub fn keys(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Surface::Palette => vec![("↑ ↓", "move"), ("Enter", "run"), ("Esc", "close")],
            Surface::Menu => vec![("↑ ↓", "move"), ("Enter", "choose"), ("Esc", "close")],
            Surface::Form => vec![("Tab", "next"), ("Enter", "press"), ("Esc", "cancel")],
            Surface::Sheet => vec![("Tab", "next"), ("Enter", "press"), ("Esc", "close")],
            Surface::Confirm => vec![("Enter", "confirm"), ("Esc", "cancel")],
            Surface::FullSize => vec![("↑ ↓", "scroll"), ("Esc", "back")],
            Surface::Filter => {
                let mut keys = crate::model::artifacts::KEYS.to_vec();
                keys.push(("Esc", "clear"));
                keys
            }
        }
    }

    /// Whether Go to (`^K`) still answers over it: only the filter leaves the global chords be.
    pub fn reaches_past(self) -> bool {
        self == Surface::Filter
    }
}

/// The keys that walk a menu the app draws itself (the keyboard's More menu), as NSH's popup
/// menus walk theirs.
pub const MENU_WALK: [&str; 4] = ["up", "down", "home", "end"];

/// Where `key` moves the keyboard focus from item `at` of a menu of `len`: Up and Down to the
/// previous and next item, around the ends, Home and End to the first and last.
pub fn menu_step(key: &str, at: usize, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(match key {
        "up" if at == 0 || at > last => last,
        "up" => at - 1,
        "down" if at >= last => 0,
        "down" => at + 1,
        "home" => 0,
        "end" => last,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_and_space_are_never_list_keys() {
        for destination in Destination::ALL {
            let keys = list_keys_live(destination, None);
            assert!(!keys.contains(&"tab") && !keys.contains(&"space"));
        }
        assert!(list_keys_live(Destination::Settings, None).is_empty());
    }

    /// Enter on a list opened the selected row's page whatever had the focus, so a focused
    /// button or another row could not be pressed with Return.
    #[test]
    fn enter_on_a_list_presses_what_has_the_focus() {
        for destination in [Destination::Home, Destination::Tasks, Destination::Sessions] {
            let keys = list_keys_live(destination, None);
            let opens = |focus: Option<Option<&str>>| {
                with_focus(keys, focus, Some("T-8")).contains(&"enter")
            };
            assert!(opens(None), "nothing focused: Enter opens the selected row");
            assert!(opens(Some(Some("T-8"))), "the selected row focused");
            assert!(!opens(Some(None)), "a button beside the list takes Enter");
            assert!(!opens(Some(Some("T-9"))), "another row takes Enter");
            assert!(with_focus(keys, Some(None), Some("T-8")).contains(&"j"));
            // With nothing selected a focused fold still takes Enter.
            assert!(!with_focus(keys, Some(None), None).contains(&"enter"));
        }
    }
}

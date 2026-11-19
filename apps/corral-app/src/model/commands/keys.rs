//! Which keys a destination's list or open page answers, and the terminal's legend of them.
//!
//! List keys (moves, Enter, digits) are the keys a focused control uses for itself: the shell
//! binds them only while the list has the keyboard focus, or nothing has, so Tab moves the focus,
//! Enter and Space press the focused control and the arrows scroll where no list wants them. The
//! legend and the bindings read the same guards as the domain reducers.

use crate::model::{Destination, Live};

const MOVE_MENU: &[&str] = &["j", "k", "down", "up", "."];
const LIVE_MOVE_OPEN: &[&str] = &["j", "k", "down", "up", "enter", "."];
const MENU: &[&str] = &["."];

/// The daemon's list keys on `destination` under the live page on top.
pub fn list_keys_live(destination: Destination, top: Option<&Live>) -> &'static [&'static str] {
    match (destination, top) {
        (Destination::Now, None | Some(Live::Task(_))) => LIVE_MOVE_OPEN,
        (Destination::Review | Destination::Agents, None) | (Destination::Plans, _) => MOVE_MENU,
        (Destination::Review, Some(Live::Review(_)))
        | (Destination::Agents, Some(Live::Session(_))) => MENU,
        _ => &[],
    }
}

/// `keys` as the keyboard focus leaves them: Enter opens the selected task only while nothing
/// has the focus or the selected task's row has it; on any other control of the list (a
/// toggle, another row, a button beside it) Enter is that control's own press. `focus`: the
/// focused control, `None` when nothing has it, `Some(None)` for one that is not a row.
pub fn with_focus(
    keys: &'static [&'static str],
    focus: Option<Option<&str>>,
    selected: Option<&str>,
) -> &'static [&'static str] {
    match focus {
        Some(row) if (row.is_none() || row != selected) && keys == LIVE_MOVE_OPEN => MOVE_MENU,
        _ => keys,
    }
}

/// The daemon-mode terminal key hints of the destination or the live page on top. `off_board`:
/// Now's selection is a task the board does not list (a closed one), whose detail already shows
/// beside the list, so Enter has no session to offer.
pub fn key_legend_live(
    destination: Destination,
    top: Option<&Live>,
    off_board: bool,
) -> Vec<(&'static str, &'static str)> {
    match (destination, top) {
        (_, Some(Live::Session(_))) => vec![(".", "session menu")],
        (_, Some(Live::Thread(_))) => Vec::new(),
        (Destination::Now, _) => {
            let mut legend = vec![("j k", "move")];
            if !off_board {
                legend.push(("Enter", "session"));
            }
            legend.extend([("]", "next tab"), ("R", "reply"), (".", "actions")]);
            legend
        }
        (Destination::Plans, _) => vec![("j k", "plan"), ("J K", "member"), (".", "actions")],
        (Destination::Agents, _) => vec![("j k", "session"), (".", "session menu")],
        (Destination::Review, _) => vec![
            ("j k", "candidate"),
            ("A", "approve"),
            ("s", "split"),
            (".", "actions"),
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
    /// A list of choices: the keyboard's Actions or Session menu, the rewind sheet's turns.
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

/// The keys that walk a menu the app draws itself (the keyboard's Actions and Session menus),
/// as NSH's popup menus walk theirs.
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
        let lists = [
            list_keys_live(Destination::Now, None),
            list_keys_live(Destination::Plans, None),
            list_keys_live(Destination::Agents, None),
        ];
        for keys in lists {
            assert!(!keys.contains(&"tab") && !keys.contains(&"space"));
        }
        assert!(list_keys_live(Destination::Settings, None).is_empty());
    }

    /// Enter in Now opened the selected task whatever had the focus, so a focused button or
    /// another row could not be pressed with Return.
    #[test]
    fn enter_in_now_presses_what_has_the_focus() {
        let keys = list_keys_live(Destination::Now, None);
        let opens =
            |focus: Option<Option<&str>>| with_focus(keys, focus, Some("T-8")).contains(&"enter");
        assert!(
            opens(None),
            "nothing focused: Enter opens the selected task"
        );
        assert!(opens(Some(Some("T-8"))), "the selected row focused");
        assert!(!opens(Some(None)), "a button beside the list takes Enter");
        assert!(!opens(Some(Some("T-9"))), "another row takes Enter");
        assert!(with_focus(keys, Some(None), Some("T-8")).contains(&"j"));
        // With nothing selected a focused toggle (Now's Waiting line) still takes Enter.
        assert!(!with_focus(keys, Some(None), None).contains(&"enter"));
    }
}

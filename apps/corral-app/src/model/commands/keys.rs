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
        Some(row) if row != selected && keys == LIVE_MOVE_OPEN => MOVE_MENU,
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
        (Destination::Artifacts, _) => crate::model::artifacts::KEYS.to_vec(),
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
    }
}

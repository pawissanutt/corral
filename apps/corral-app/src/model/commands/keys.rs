//! Which keys a destination's list or open page answers, and the terminal's legend of them.
//!
//! List keys (moves, Enter, digits) are the keys a focused control uses for itself: the shell
//! binds them only while the list has the keyboard focus, or nothing has, so Tab moves the focus,
//! Enter and Space press the focused control and the arrows scroll where no list wants them. The
//! legend and the bindings read the same guards as the domain reducers.

use crate::model::{Destination, Live};

const MOVE: &[&str] = &["j", "k", "down", "up"];
const LIVE_MOVE_OPEN: &[&str] = &["j", "k", "down", "up", "enter", "."];
const MENU: &[&str] = &["."];
const LIVE_QUESTION: &[&str] = &["j", "k", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const LIVE_INBOX: &[&str] = &[
    "j", "k", "down", "up", "enter", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

/// The daemon's list keys on `destination` under the live page on top.
pub fn list_keys_live(destination: Destination, top: Option<&Live>) -> &'static [&'static str] {
    match (destination, top) {
        (Destination::Inbox, Some(Live::Question(_))) => LIVE_QUESTION,
        (Destination::Inbox, None) => LIVE_INBOX,
        (Destination::Now, None | Some(Live::Task(_))) => LIVE_MOVE_OPEN,
        (Destination::Review, None | Some(Live::Review(_))) => MENU,
        (Destination::Plans, _) => MOVE,
        _ => &[],
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
        (_, Some(Live::Question(_))) => {
            vec![("1-9", "choose"), ("j k", "move"), ("e", "evidence")]
        }
        (_, Some(Live::Session(_))) => vec![("`", "transcript/terminal")],
        (_, Some(Live::Thread(_))) => Vec::new(),
        (Destination::Inbox, _) => vec![("j k", "move"), ("Enter", "open"), ("1-9", "choose")],
        (Destination::Now, _) => {
            let mut legend = vec![("j k", "move")];
            if !off_board {
                legend.push(("Enter", "session"));
            }
            legend.extend([("]", "next tab"), (".", "actions")]);
            legend
        }
        (Destination::Plans, _) => vec![("j k", "plan"), ("J K", "member")],
        (Destination::Review, _) => vec![("s", "split"), (".", "actions")],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_and_space_are_never_list_keys() {
        let lists = [
            list_keys_live(Destination::Inbox, None),
            list_keys_live(
                Destination::Inbox,
                Some(&Live::Question("T-1/R1/Q1".into())),
            ),
            list_keys_live(Destination::Now, None),
            list_keys_live(Destination::Plans, None),
        ];
        for keys in lists {
            assert!(!keys.contains(&"tab") && !keys.contains(&"space"));
        }
        assert!(list_keys_live(Destination::Agents, None).is_empty());
        assert!(list_keys_live(Destination::Settings, None).is_empty());
    }
}

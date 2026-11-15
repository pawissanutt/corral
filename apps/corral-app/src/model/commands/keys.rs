//! Which keys a destination's list or open page answers, and the terminal's legend of them.
//!
//! List keys (moves, Enter, digits) are the keys a focused control uses for itself: the shell
//! binds them only while the list has the keyboard focus, or nothing has, so Tab moves the focus,
//! Enter and Space press the focused control and the arrows scroll where no list wants them. The
//! legend and the bindings read the same guards as the domain reducers.

use crate::model::{Destination, Live, Page};

const MOVE: &[&str] = &["j", "k", "down", "up"];
const MOVE_OPEN: &[&str] = &["j", "k", "down", "up", "enter"];
const LIVE_MOVE_OPEN: &[&str] = &["j", "k", "down", "up", "enter", "."];
const MENU: &[&str] = &["."];
/// The fixture's question offers two answers; the arrows stay with the page's scroll.
const FIXTURE_QUESTION: &[&str] = &["j", "k", "1", "2"];
const FIXTURE_INBOX: &[&str] = &["j", "k", "down", "up", "enter", "1", "2"];
const LIVE_QUESTION: &[&str] = &["j", "k", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const LIVE_INBOX: &[&str] = &[
    "j", "k", "down", "up", "enter", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

/// The fixture preview's list keys on `destination` under `page`.
pub fn list_keys(destination: Destination, page: Option<Page>) -> &'static [&'static str] {
    match (destination, page) {
        (Destination::Inbox, Some(Page::Question)) => FIXTURE_QUESTION,
        (Destination::Inbox, None) => FIXTURE_INBOX,
        (Destination::Now, None | Some(Page::Task(_))) => MOVE_OPEN,
        (Destination::Plans, _) => MOVE,
        _ => &[],
    }
}

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

/// Screen-local terminal key hints, using the same page guards as the domain reducers.
/// Destination navigation and global Ask/palette hints are rendered separately by the shell.
pub fn key_legend(
    destination: Destination,
    page: Option<Page>,
) -> Vec<(&'static str, &'static str)> {
    match (destination, page) {
        (_, Some(Page::Question)) => vec![("1 2", "choose"), ("j k", "move"), ("e", "evidence")],
        (Destination::Inbox, None) => vec![
            ("j k", "move"),
            ("Enter", "open"),
            ("f", "focus"),
            ("1 2", "choose"),
        ],
        (Destination::Now, None | Some(Page::Task(_))) => {
            vec![("j k", "move"), ("Enter", "session"), ("]", "next tab")]
        }
        (Destination::Plans, _) => vec![("j k", "plan"), ("J K", "member"), ("x", "scale")],
        (Destination::Review, None) => vec![("s", "split")],
        _ => Vec::new(),
    }
}

/// The daemon-mode terminal key hints of the destination or the live page on top.
pub fn key_legend_live(
    destination: Destination,
    top: Option<&Live>,
) -> Vec<(&'static str, &'static str)> {
    match (destination, top) {
        (_, Some(Live::Question(_))) => {
            vec![("1-9", "choose"), ("j k", "move"), ("e", "evidence")]
        }
        (_, Some(Live::Session(_))) => vec![("`", "transcript/terminal")],
        (_, Some(Live::Thread(_))) => Vec::new(),
        (Destination::Inbox, _) => vec![("j k", "move"), ("Enter", "open"), ("1-9", "choose")],
        (Destination::Now, _) => vec![
            ("j k", "move"),
            ("Enter", "session"),
            ("]", "next tab"),
            (".", "actions"),
        ],
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
            list_keys(Destination::Inbox, None),
            list_keys(Destination::Inbox, Some(Page::Question)),
            list_keys(Destination::Now, None),
            list_keys(Destination::Plans, None),
            list_keys_live(Destination::Inbox, None),
            list_keys_live(Destination::Now, None),
        ];
        for keys in lists {
            assert!(!keys.contains(&"tab") && !keys.contains(&"space"));
        }
        assert!(list_keys(Destination::Agents, None).is_empty());
        assert!(list_keys_live(Destination::Settings, None).is_empty());
    }
}

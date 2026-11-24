//! What a task's change did to the map's nodes and edges (a task page's Changes › Impact): a
//! node it touched, with its lines added and removed and the files they are in, and the
//! dependents it holds, code outside the change that references a changed definition. A
//! package can be both. A node with no mark is untouched and drawn dimmed with its own figure.
//! A node that stands for files the level does not list (a "N more" card, a neighbouring
//! package) carries their total. An edge whose reference count the change moved carries each
//! module pair it folds, before and after; every other edge is quiet.

use std::collections::BTreeMap;

use super::scene::{Card, Scene};
use super::words;
use super::{Node, Overlay, Pick, Window, count, plural};
use crate::model::hue::Hue;
use crate::model::text::{Rich, flat, span};

/// A touched node's solid ring: the owner's colour, as the changed edges.
pub const TOUCHED: Hue = Hue::You;
/// A node holding dependents: a dashed ring and a chip in Corral's colour, so it differs from a
/// touched one in shape and in colour. The map's other colours mean other things (a running
/// worker, a reference no dependency declares), so neither mark borrows them.
pub const DEPENDENT: Hue = Hue::Corral;
/// An edge whose reference count the change moved.
pub const CHANGED: Hue = Hue::You;
pub const ADDED: Hue = Hue::Ok;
pub const REMOVED: Hue = Hue::Fail;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The change touched the node: its files, or a definition in it.
    pub touched: bool,
    pub added: u64,
    pub removed: u64,
    /// Changed files under the node that the head still has, and their lines: what a node
    /// standing for them says, so the churn and the count describe the same files.
    pub files: u64,
    pub file_added: u64,
    pub file_removed: u64,
    /// Every changed file under the node, deleted ones included: the files `added` and
    /// `removed` count lines in, so a card's churn and its file count describe the same set.
    pub changed: u64,
    /// Definitions here, outside the change, that reference a changed one.
    pub dependents: u64,
}

pub type Marks = BTreeMap<Node, Mark>;

/// One module pair's reference count, before and after the change; `from` and `to` as said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub from: String,
    pub to: String,
    pub before: u64,
    pub after: u64,
}

/// An edge's label under a change: `before → after` for one pair, `N changes` for several.
pub fn change_words(deltas: &[Delta]) -> Option<String> {
    match deltas {
        [] => None,
        [one] => Some(format!("{} → {}", count(one.before), count(one.after))),
        many => Some(plural(many.len() as u64, "change", "changes")),
    }
}

/// An edge's change as spoken: each pair with its before and after.
pub fn change_spoken(deltas: &[Delta]) -> Option<String> {
    match deltas {
        [] => None,
        [one] => Some(format!(
            "changed: {} → {} from {} to {} references",
            one.from,
            one.to,
            count(one.before),
            count(one.after)
        )),
        many => Some(format!(
            "{} changed: {}",
            plural(many.len() as u64, "module pair", "module pairs"),
            many.iter()
                .map(|d| format!(
                    "{} → {} from {} to {}",
                    d.from,
                    d.to,
                    count(d.before),
                    count(d.after)
                ))
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

/// Lines added and removed, each in its colour.
fn churn(added: u64, removed: u64) -> Rich {
    vec![
        span(format!("+{}", count(added))).hue(ADDED),
        span(" "),
        span(format!("−{}", count(removed))).hue(REMOVED),
    ]
}

impl Mark {
    pub fn churn(self) -> u64 {
        self.added + self.removed
    }

    /// The dependents the node holds, said apart from its figure (a card's chip), so neither
    /// crowds the other off its line.
    pub fn dependent_words(self) -> Option<String> {
        (self.dependents > 0).then(|| plural(self.dependents, "dependent", "dependents"))
    }

    /// What the change did to the node itself: its lines added and removed and, for a node
    /// holding files, how many changed; nothing where it touched none.
    fn change(self, node: &Node) -> Rich {
        if !self.touched {
            return Vec::new();
        }
        if self.churn() == 0 {
            return vec![span("changed")];
        }
        let mut said = churn(self.added, self.removed);
        // A file's count would only say "1 file".
        if self.changed > 0 && !matches!(node, Node::File(_)) {
            said.push(span(format!(
                " · {}",
                plural(self.changed, "file", "files")
            )));
        }
        said
    }

    /// The change of a node that stands for files the level does not list.
    fn hidden(self, node: &Node) -> Rich {
        if self.files == 0 {
            return self.change(node);
        }
        let mut said = churn(self.file_added, self.file_removed);
        said.push(span(format!(
            " in {} not listed",
            plural(self.files, "file", "files")
        )));
        said
    }

    /// The mark as a screen reader says it after the node's name.
    pub fn spoken(self) -> String {
        let mut said = Vec::new();
        if self.touched {
            said.push(if self.churn() > 0 {
                format!(
                    "changed: {} added, {} deleted",
                    count(self.added),
                    count(self.removed)
                )
            } else {
                "changed".to_owned()
            });
        }
        if self.dependents > 0 {
            said.push(plural(self.dependents, "dependent", "dependents"));
        }
        said.join(", ")
    }
}

/// A node's mark; a neighbouring package on a package's boundary carries the package's.
pub fn of<'a>(marks: &'a Marks, node: &Node) -> Option<&'a Mark> {
    match node {
        Node::Port { path, .. } => marks.get(&Node::Container(path.clone())),
        _ => marks.get(node),
    }
}

/// An unmarked card as spoken; its figure stays its own.
pub const UNTOUCHED: &str = "untouched";

/// What the change did to `node` under `mark`, each run in its colour: the hidden files' total
/// where the node stands for files the level does not list. Empty where the change touched
/// nothing of it (untouched, or only holding dependents).
pub fn change_of(node: &Node, mark: Option<Mark>) -> Rich {
    match (node, mark) {
        (_, None) => Vec::new(),
        (Node::More | Node::Port { .. }, Some(mark)) => mark.hidden(node),
        (_, Some(mark)) => mark.change(node),
    }
}

/// A card's change figure as words; empty where the card keeps its own.
pub fn figure(marks: &Marks, node: &Node) -> String {
    flat(&change_of(node, of(marks, node).copied()))
}

/// A card's figure line under marks: the change, else its size (the Structure overlay's
/// figure), which helps judge how far an untouched or depending node reaches.
pub fn figure_line(card: &Card, mark: Option<Mark>) -> Rich {
    let change = change_of(&card.node, mark);
    if change.is_empty() {
        vec![span(words::figure(
            card,
            Overlay::Structure,
            Window::Days30,
        ))]
    } else {
        change
    }
}

/// A port's one line under marks: its figure, then the dependents its package holds, since a
/// pill has no room for a chip.
pub fn port_line(card: &Card, mark: Option<Mark>) -> Rich {
    let mut line = figure_line(card, mark);
    if let Some(words) = mark.and_then(Mark::dependent_words) {
        line.push(span(" · "));
        line.push(span(words).hue(DEPENDENT));
    }
    line
}

/// A card's mark as spoken.
pub fn spoken(marks: &Marks, node: &Node) -> String {
    match (node, of(marks, node)) {
        (_, None) => UNTOUCHED.to_owned(),
        (Node::More | Node::Port { .. }, Some(mark)) if mark.files > 0 => {
            let mut said = format!(
                "changed: {} added, {} deleted, in {} not listed",
                count(mark.file_added),
                count(mark.file_removed),
                plural(mark.files, "file", "files")
            );
            if mark.dependents > 0 {
                said.push_str(&format!(
                    ", {}",
                    plural(mark.dependents, "dependent", "dependents")
                ));
            }
            said
        }
        (_, Some(mark)) => mark.spoken(),
    }
}

/// The pick as a task's map draws it: a file or definition the level folds into its "N more"
/// card is shown by that card when the card carries the change, so following an item never
/// leaves its only mark dimmed.
pub fn drawn_pick(scene: &Scene, marks: &Marks, picked: Option<Pick>) -> Option<Pick> {
    match picked {
        Some(Pick::Node(node @ (Node::File(_) | Node::Definition { .. })))
            if scene.card(&node).is_none()
                && scene.card(&Node::More).is_some()
                && marks.contains_key(&Node::More) =>
        {
            Some(Pick::Node(Node::More))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::system::scene::Kind;

    /// Impact put the word "untouched" on every unmarked card in place of its size, drew a
    /// touched card's churn in plain ink, and set a node's dependents on its figure line, where
    /// they were cut.
    #[test]
    fn untouched_cards_keep_their_size_and_churn_is_coloured_apart_from_dependents() {
        let mut card = Card::new(Node::Container("a".into()), "a".into(), Kind::Package);
        card.lines = 120;
        let size = vec![span(words::figure(
            &card,
            Overlay::Structure,
            Window::Days30,
        ))];
        assert_eq!(figure_line(&card, None), size);
        let held = Mark {
            dependents: 3,
            ..Mark::default()
        };
        assert_eq!(figure_line(&card, Some(held)), size);
        let both = Mark {
            touched: true,
            added: 4,
            removed: 1,
            changed: 2,
            ..held
        };
        let line = figure_line(&card, Some(both));
        assert!(line.iter().any(|run| run.hue == ADDED));
        assert!(line.iter().any(|run| run.hue == REMOVED));
        assert!(line.iter().all(|run| run.hue != DEPENDENT));
        assert!(both.dependent_words().is_some());
    }
}

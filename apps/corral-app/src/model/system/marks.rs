//! What a task's change did to the map's nodes and edges (a task page's Changes › Impact): a
//! node it touched, with its lines added and removed and the files they are in, and the
//! dependents it holds, code outside the change that references a changed definition. A
//! package can be both. A node with no mark is untouched and drawn dimmed. A node that stands
//! for files the level does not list (a "N more" card, a neighbouring package) carries their
//! total. An edge whose reference count the change moved carries each module pair it folds,
//! before and after; every other edge is quiet.

use std::collections::BTreeMap;

use super::scene::Scene;
use super::{Node, Pick, count, plural};

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

impl Mark {
    pub fn churn(self) -> u64 {
        self.added + self.removed
    }

    fn dependent_words(self) -> Option<String> {
        (self.dependents > 0).then(|| {
            format!(
                "{} dependent",
                plural(self.dependents, "definition", "definitions")
            )
        })
    }

    /// The figure a marked card shows in place of its overlay's.
    pub fn words(self) -> String {
        let mut said = Vec::new();
        if self.touched {
            said.push(if self.churn() > 0 {
                format!("+{} −{}", count(self.added), count(self.removed))
            } else {
                "changed".to_owned()
            });
        }
        said.extend(self.dependent_words());
        said.join(" · ")
    }

    /// The figure of a node that stands for files the level does not list.
    pub fn hidden_words(self) -> String {
        if self.files == 0 {
            return self.words();
        }
        let mut said = vec![format!(
            "+{} −{} in {} not listed",
            count(self.file_added),
            count(self.file_removed),
            plural(self.files, "file", "files")
        )];
        said.extend(self.dependent_words());
        said.join(" · ")
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

/// An unmarked card's figure.
pub const UNTOUCHED: &str = "untouched";

/// The figure of `node` under `mark`: the hidden files' total where the node stands for files
/// the level does not list.
pub fn words_of(node: &Node, mark: Option<Mark>) -> String {
    match (node, mark) {
        (_, None) => UNTOUCHED.to_owned(),
        (Node::More | Node::Port { .. }, Some(mark)) => mark.hidden_words(),
        (_, Some(mark)) => mark.words(),
    }
}

/// A card's figure under marks.
pub fn figure(marks: &Marks, node: &Node) -> String {
    words_of(node, of(marks, node).copied())
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

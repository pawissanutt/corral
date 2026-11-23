//! What a task's change did to the map's nodes (a task page's Changes › Impact): a node it
//! touched, with its lines added and removed, and the dependents it holds, code outside the
//! change that references a changed definition. A package can be both. A node with no mark is
//! untouched and drawn dimmed.

use std::collections::BTreeMap;

use super::{Node, count, plural};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The change touched the node: its files, or a definition in it.
    pub touched: bool,
    pub added: u64,
    pub removed: u64,
    /// Definitions here, outside the change, that reference a changed one.
    pub dependents: u64,
}

pub type Marks = BTreeMap<Node, Mark>;

impl Mark {
    pub fn churn(self) -> u64 {
        self.added + self.removed
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
        if self.dependents > 0 {
            said.push(format!(
                "{} dependent",
                plural(self.dependents, "definition", "definitions")
            ));
        }
        said.join(" · ")
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

/// A card's figure under marks.
pub fn figure(marks: &Marks, node: &Node) -> String {
    of(marks, node).map_or_else(|| UNTOUCHED.to_owned(), |mark| mark.words())
}

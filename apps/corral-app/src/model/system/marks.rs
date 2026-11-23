//! What a task's change did to the map's nodes (a task page's Changes › Impact): a node it
//! touched, with its lines added and removed, or a dependent, holding code outside the change
//! that references a changed definition. A node with no mark is untouched and drawn dimmed.

use std::collections::BTreeMap;

use super::{Node, count, plural};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Touched {
        added: u64,
        removed: u64,
    },
    /// Definitions outside the change that reference a changed one.
    Dependent {
        definitions: u64,
    },
}

pub type Marks = BTreeMap<Node, Mark>;

impl Mark {
    /// The figure a marked card shows in place of its overlay's.
    pub fn words(self) -> String {
        match self {
            Mark::Touched { added, removed } => format!("+{} −{}", count(added), count(removed)),
            Mark::Dependent { definitions } => {
                format!(
                    "dependent · {}",
                    plural(definitions, "definition", "definitions")
                )
            }
        }
    }
}

/// An unmarked card's figure.
pub const UNTOUCHED: &str = "untouched";

/// A card's figure under marks.
pub fn figure(marks: &Marks, node: &Node) -> String {
    marks
        .get(node)
        .map_or_else(|| UNTOUCHED.to_owned(), |mark| mark.words())
}

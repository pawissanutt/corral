//! The impact view in the address and under Escape.

use corral_types::{ModuleId, SystemAt};

use crate::model::address::{escape_part, unescape};
use crate::model::system::{Action, Node, Pick};

use super::{State, View};

impl State {
    /// The next Escape action in Impact, if it would keep the owner on the task page.
    pub fn escape_hint(&self) -> Option<&'static str> {
        if self.view != View::Impact {
            return None;
        }
        if self.has_selection() {
            Some("clear")
        } else if self.map.at().is_some() {
            Some("up")
        } else {
            None
        }
    }

    /// Lets go of the selection before ascending; `false` lets the caller leave the page.
    pub fn escape(&mut self) -> bool {
        if self.view != View::Impact {
            return false;
        }
        if self.has_selection() {
            self.item = None;
            self.map.picked = None;
            true
        } else {
            self.map.act(Action::Up)
        }
    }

    fn has_selection(&self) -> bool {
        self.item.is_some() || self.map.picked.is_some()
    }

    /// The picture the browser can name for this task, not the last task's map.
    pub fn address(&self, task: &str) -> Address {
        if self.task.as_deref() != Some(task) {
            return Address::default();
        }
        Address {
            trail: self.map.trail.clone(),
            picked: self.map.picked.clone(),
            item: self.item.clone(),
        }
    }

    /// Restores the picture from a link without changing the task's list folds.
    pub fn open_address(&mut self, task: &str, address: &Address) {
        self.follow(task);
        self.view = View::Impact;
        self.map.trail.clone_from(&address.trail);
        self.map.picked.clone_from(&address.picked);
        self.item.clone_from(&address.item);
    }
}

/// The authored place on the task's map, separate from its folded lists and other local UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Address {
    trail: Vec<SystemAt>,
    picked: Option<Pick>,
    item: Option<String>,
}

impl Address {
    pub(crate) fn query(&self) -> String {
        let mut fields = Vec::new();
        if !self.trail.is_empty() {
            // Keeping the trail, not only its deepest level, preserves breadcrumbs and Up.
            let trail = self
                .trail
                .iter()
                .map(level_key)
                .collect::<Vec<_>>()
                .join("!");
            fields.push(format!("at={trail}"));
        }
        if let Some(picked) = self.picked.as_ref().and_then(pick_key) {
            fields.push(format!("sel={picked}"));
        }
        if let Some(item) = &self.item {
            fields.push(format!("item={}", escape_part(item)));
        }
        fields.join("&")
    }

    pub(crate) fn from_query(query: &str) -> Option<Self> {
        let mut address = Self::default();
        for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
            match key {
                "at" => {
                    address.trail = value
                        .split('!')
                        .map(|part| match node_from_key(part)? {
                            node @ (Node::Container(_) | Node::Module(_) | Node::File(_)) => {
                                node.opens()
                            }
                            _ => None,
                        })
                        .collect::<Option<_>>()?;
                }
                "sel" => {
                    address.picked = Some(if let Some((from, to)) = value.split_once('!') {
                        Pick::Edge(node_from_key(from)?, node_from_key(to)?)
                    } else {
                        Pick::Node(node_from_key(value)?)
                    });
                }
                "item" => address.item = Some(unescape(value)?),
                _ => {}
            }
        }
        Some(address)
    }
}

fn pick_key(pick: &Pick) -> Option<String> {
    match pick {
        Pick::Node(node) => node_key(node),
        Pick::Edge(from, to) => Some(format!("{}!{}", node_key(from)?, node_key(to)?)),
    }
}

fn level_key(at: &SystemAt) -> String {
    match at {
        SystemAt::Container { path } => format!("c:{}", escape_part(path)),
        SystemAt::Module { module } => module_key(module),
        SystemAt::File { path } => format!("f:{}", escape_part(path)),
    }
}

fn module_key(module: &ModuleId) -> String {
    format!(
        "m:{}:{}",
        escape_part(&module.container),
        escape_part(&module.path)
    )
}

fn node_key(node: &Node) -> Option<String> {
    Some(match node {
        Node::Container(path) => format!("c:{}", escape_part(path)),
        Node::Port { path, into } => {
            format!("{}:{}", if *into { "i" } else { "o" }, escape_part(path))
        }
        Node::Module(module) => module_key(module),
        Node::File(path) => format!("f:{}", escape_part(path)),
        Node::Definition { line, name } => format!("d:{line}:{}", escape_part(name)),
        Node::More => "more".into(),
        // These nodes only exist on System's other views, never in review geometry.
        Node::Unit(_) | Node::Scripts | Node::Type(_) => return None,
    })
}

fn node_from_key(key: &str) -> Option<Node> {
    if key == "more" {
        return Some(Node::More);
    }
    let (kind, rest) = key.split_once(':')?;
    Some(match kind {
        "c" => Node::Container(unescape(rest)?),
        "i" | "o" => Node::Port {
            path: unescape(rest)?,
            into: kind == "i",
        },
        "m" => {
            let (container, path) = rest.split_once(':')?;
            Node::Module(ModuleId {
                container: unescape(container)?,
                path: unescape(path)?,
            })
        }
        "f" => Node::File(unescape(rest)?),
        "d" => {
            let (line, name) = rest.split_once(':')?;
            Node::Definition {
                line: line.parse().ok()?,
                name: unescape(name)?,
            }
        }
        _ => return None,
    })
}

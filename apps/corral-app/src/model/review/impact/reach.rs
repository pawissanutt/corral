//! Where "Open in System" can take an item. Activity › System draws the project at its current
//! head, not at the task's submission, so a package or file the change adds is not there until
//! it lands. Each level along the item's target is looked up in Activity › System's own served
//! map: the item opens at its node when the map has it, at the deepest level that holds it
//! otherwise, and says so when the map has none of it.

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::{SystemAt, SystemStatus};

use super::OPEN_IN_SYSTEM;
use super::items::Target;
use crate::model::daemon::Feed;
use crate::model::system::scene::{build, module_name};
use crate::model::system::{Node, base_name, request};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Activity › System has `node` at `trail`; `note` says what it stands for when it is not
    /// the item's own node.
    Open {
        trail: Vec<SystemAt>,
        node: Node,
        label: String,
        note: Option<String>,
    },
    /// Activity › System's map is still being read.
    Checking,
    Unavailable(String),
}

pub const CHECKING: &str = "Looking for it on Activity › System's map…";

/// The System maps a target is looked up in, one per level along it.
pub fn wanted(project: &str, target: &Target) -> Vec<DetailRef> {
    (0..=target.trail.len())
        .map(|depth| request(project, target.trail[..depth].last()))
        .collect()
}

fn words(node: &Node) -> String {
    match node {
        Node::Container(path) | Node::Port { path, .. } if path.is_empty() => "(root)".into(),
        Node::Container(path) | Node::Port { path, .. } => base_name(path).to_owned(),
        Node::Module(module) => module_name(module),
        Node::File(path) => base_name(path).to_owned(),
        Node::Definition { name, .. } => name.clone(),
        Node::More => "more".into(),
    }
}

pub fn reach(feed: &Feed, project: &str, target: &Target) -> Reach {
    let trail = &target.trail;
    let mut found: Option<(usize, Node, String)> = None;
    for depth in 0..=trail.len() {
        let node = match trail.get(depth) {
            Some(at) => Node::of(at),
            None => target.node.clone(),
        };
        let at = trail[..depth].last();
        let cache = feed.detail(&request(project, at));
        let error = cache.and_then(|cache| cache.error.as_ref());
        let map = match (cache.and_then(|cache| cache.body()), error) {
            (Some(DetailBody::SystemMap(map)), _) => map,
            // A level System cannot find at its head does not hold the node.
            (_, Some(error)) if error.code == "not_found" => break,
            (_, Some(error)) => {
                return Reach::Unavailable(format!(
                    "Activity › System's map could not be read: {}",
                    error.message
                ));
            }
            _ => return Reach::Checking,
        };
        // Its facts are still being built: it may yet have the node.
        if map.status == SystemStatus::Building && map.rows.is_empty() {
            return Reach::Checking;
        }
        let scene = build(map, at);
        let Some(card) = scene.card(&node) else {
            break;
        };
        found = Some((depth, node, card.name.clone()));
    }
    let wanted = words(&target.node);
    match found {
        None => Reach::Unavailable(format!(
            "Not on Activity › System's map: it draws {project} at its current head, where \
             {wanted} is not (this change has not landed there)."
        )),
        Some((depth, node, _)) if depth == trail.len() => Reach::Open {
            trail: trail.clone(),
            node,
            label: OPEN_IN_SYSTEM.into(),
            note: None,
        },
        Some((depth, node, name)) => Reach::Open {
            trail: trail[..depth].to_vec(),
            label: format!("Open {name} in System"),
            note: Some(format!(
                "Activity › System's map at the current head does not show {wanted}; {name}, \
                 which holds it, is there."
            )),
            node,
        },
    }
}

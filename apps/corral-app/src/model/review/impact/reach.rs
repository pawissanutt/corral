//! Where "Open in System" can take an item. Activity › System draws the project at its own
//! head, not at the task's submission, so it need not have the item's node. Each level along
//! the item's target is looked up in Activity › System's own served map, confirmed and read at
//! one head: the item opens at its node when the map has it, at the deepest level that holds it
//! otherwise, and says so when the map has none of it. Until every level is confirmed it says
//! it is still checking, and what it could not read.

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
        Node::Unit(id) => id.name.clone(),
        Node::Scripts => "npm scripts".into(),
    }
}

pub fn reach(feed: &Feed, project: &str, target: &Target) -> Reach {
    let trail = &target.trail;
    let mut found: Option<(usize, Node, String)> = None;
    let mut head: Option<Option<String>> = None;
    for depth in 0..=trail.len() {
        let node = match trail.get(depth) {
            Some(at) => Node::of(at),
            None => target.node.clone(),
        };
        let at = trail[..depth].last();
        let Some(cache) = feed.detail(&request(project, at)) else {
            return Reach::Checking;
        };
        // A failed read is kept stale with its error and waits for a retry: say so first.
        match &cache.error {
            // A level System cannot find at its head does not hold the node.
            Some(error) if error.code == "not_found" => break,
            Some(error) => {
                return Reach::Unavailable(format!(
                    "Could not check Activity › System's map: {}",
                    error.message
                ));
            }
            None => {}
        }
        // Only a confirmed level can promise the node: a kept body may be another head's.
        let map = match cache.body() {
            Some(DetailBody::SystemMap(map)) if !cache.loading && !cache.stale => map,
            _ => return Reach::Checking,
        };
        // Its facts are still being built: it may yet have the node.
        if map.status == SystemStatus::Building && map.rows.is_empty() {
            return Reach::Checking;
        }
        // Levels read at different heads do not describe one map. A System change marks every
        // level of the project stale (`client/app/rows.rs`), so the keeper reads the older again.
        match &head {
            Some(head) if *head != map.head => return Reach::Checking,
            _ => head = Some(map.head.clone()),
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
            "Not on Activity › System's map: {wanted} is not in {project}{}.",
            head.flatten()
                .map(|head| format!(" at {}", head.get(..10).unwrap_or(&head)))
                .unwrap_or_default()
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
                "Activity › System's map does not show {wanted}; {name}, which holds it, is \
                 there."
            )),
            node,
        },
    }
}

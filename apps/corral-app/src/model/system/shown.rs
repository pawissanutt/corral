//! What the page shows from the selected feed: the project in scope (or the projects to choose
//! from), the level's served map as a scene, where its read stands, the breadcrumb, the
//! notices, the summary, the legend, the inspector and the provenance line.

use corral_protocol::detail::DetailBody;
use corral_types::{SystemAt, SystemData, SystemDataRow, SystemMap, SystemRow, SystemRuntime};

use super::inspect::{Inspector, inspect};
use super::scene::{Scene, build, module_name};
use super::tree::matched;
use super::words::{Notice, Swatch, legend, notices, provenance, summary};
use super::{Node, State, View, base_name, data_request, request, runtime_request};
use super::{data, runtime};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed};
use crate::model::projects::{Scope, known_projects};

pub const ASK_HEADING: &str = "Choose a project";
pub const ASK_WORDS: &str =
    "The map is one project's packages, modules, files and definitions. Pick the project to draw.";

/// Where the level's read stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// The last complete body, kept while corrald is unreachable or revalidating failed.
    Stale,
    Loading,
    Failed(String),
}

fn status_of(cache: Option<&DetailCache>) -> Status {
    let Some(cache) = cache else {
        return Status::Loading;
    };
    match (cache.body(), &cache.error) {
        (None, Some(error)) => Status::Failed(error.message.clone()),
        (None, None) => Status::Loading,
        (Some(_), Some(_)) => Status::Stale,
        (Some(_), None) if cache.stale => Status::Stale,
        (Some(_), None) => Status::Ready,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// The project drawn; none while the scope is All projects.
    pub project: Option<String>,
    /// The projects the owner can choose, when none is in scope.
    pub projects: Vec<Id>,
    pub status: Status,
    pub scene: Option<Scene>,
    /// The breadcrumb: the project, then each level drilled into.
    pub crumbs: Vec<String>,
    pub notices: Vec<Notice>,
    pub summary: Vec<String>,
    pub legend: Vec<(Swatch, String)>,
    pub inspector: Option<Inspector>,
    /// The filter's matches at this level, of how many.
    pub matched: Option<(usize, usize)>,
    pub provenance: String,
    /// Said when the level is served with nothing on it.
    pub empty: Option<&'static str>,
    /// What the inspector says while nothing is picked.
    pub hint: &'static str,
}

pub const HINT: &str = "Select a package, module, file or edge for its facts. Select it again to open it; Escape goes up.";
pub const DATA_HINT: &str = "Select a type for its fields, where it is declared and the types that hold it, or a has edge for its fields.";
pub const RUNTIME_HINT: &str = "Select a unit, a package or an edge for where it is declared, its entry points and what starts it.";

/// The served map of one level, if any.
pub fn map_of<'a>(feed: &'a Feed, project: &str, at: Option<&SystemAt>) -> Option<&'a SystemMap> {
    match feed.detail(&request(project, at))?.body()? {
        DetailBody::SystemMap(map) => Some(map),
        _ => None,
    }
}

fn crumb(feed: &Feed, project: &str, at: &SystemAt) -> String {
    match at {
        SystemAt::Container { path } => map_of(feed, project, None)
            .map(|map| build(map, None))
            .and_then(|scene| {
                scene
                    .card(&Node::Container(path.clone()))
                    .map(|c| c.name.clone())
            })
            .unwrap_or_else(|| {
                if path.is_empty() {
                    "(root)".into()
                } else {
                    base_name(path).into()
                }
            }),
        SystemAt::Module { module } => module_name(module),
        SystemAt::File { path } => base_name(path).into(),
    }
}

pub fn shown(state: &State, feed: &Feed, scope: &Scope) -> Shown {
    let project = match scope.resolved(feed) {
        Scope::Project(id) => Some(id.to_string()),
        Scope::All => None,
    };
    let Some(project) = project else {
        return Shown {
            project: None,
            projects: known_projects(feed),
            status: Status::Ready,
            scene: None,
            crumbs: Vec::new(),
            notices: Vec::new(),
            summary: Vec::new(),
            legend: Vec::new(),
            inspector: None,
            matched: None,
            provenance: String::new(),
            empty: None,
            hint: HINT,
        };
    };
    // A trail kept from another project would read a level that project does not have.
    let trail: &[SystemAt] = if state.project.as_deref() == Some(project.as_str()) {
        &state.trail
    } else {
        &[]
    };
    let at = trail.last();
    let row: Option<&SystemRow> = feed.systems.get(project.as_str());
    if state.view == View::Runtime {
        return runtime_shown(state, feed, project, row);
    }
    if state.view == View::Data {
        return data_shown(state, feed, project, row);
    }
    // The Lenses view draws from its own reads (`screens::system::lenses`).
    if state.view == View::Lenses {
        return Shown {
            project: Some(project.clone()),
            projects: Vec::new(),
            status: Status::Ready,
            scene: None,
            crumbs: vec![project],
            notices: Vec::new(),
            summary: Vec::new(),
            legend: Vec::new(),
            inspector: None,
            matched: None,
            provenance: String::new(),
            empty: None,
            hint: super::lens::HINT,
        };
    }
    let status = status_of(feed.detail(&request(&project, at)));
    let map = map_of(feed, &project, at);
    let scene = map.map(|map| build(map, at));
    let mut crumbs = vec![project.clone()];
    crumbs.extend(trail.iter().map(|at| crumb(feed, &project, at)));
    let empty = match (&scene, map) {
        (Some(scene), Some(map)) if scene.cards.is_empty() && map.error.is_none() => {
            Some(match at {
                None => "No packages or folders found at this commit.",
                Some(SystemAt::Container { .. }) => "No modules found in this package.",
                Some(SystemAt::Module { .. }) => "No files found in this module.",
                Some(SystemAt::File { .. }) => {
                    "No definitions: a facts extension supplies them, and none has for this file."
                }
            })
        }
        _ => None,
    };
    Shown {
        projects: Vec::new(),
        status,
        crumbs,
        notices: notices(map, row),
        summary: match (&scene, map) {
            (Some(scene), Some(map)) => summary(scene, map, state.overlay, state.window),
            _ => Vec::new(),
        },
        legend: scene
            .as_ref()
            .map(|scene| legend(scene, state.overlay, state.window))
            .unwrap_or_default(),
        inspector: scene
            .as_ref()
            .zip(state.picked.as_ref())
            .and_then(|(scene, pick)| inspect(scene, pick, at)),
        matched: scene
            .as_ref()
            .filter(|_| !state.filter.trim().is_empty())
            .map(|scene| matched(scene, &state.filter)),
        provenance: provenance(map, row),
        empty,
        scene,
        project: Some(project),
        hint: HINT,
    }
}

/// The served `system_runtime`, if any.
pub fn runtime_of<'a>(feed: &'a Feed, project: &str) -> Option<&'a SystemRuntime> {
    match feed.detail(&runtime_request(project))?.body()? {
        DetailBody::SystemRuntime(runtime) => Some(runtime),
        _ => None,
    }
}

/// The served `system_data` at the project head, if any.
pub fn data_of<'a>(feed: &'a Feed, project: &str) -> Option<&'a SystemData> {
    match feed.detail(&data_request(project))?.body()? {
        DetailBody::SystemData(data) => Some(data),
        _ => None,
    }
}

/// The Data view: one level, the types by container and the types their fields hold.
fn data_shown(state: &State, feed: &Feed, project: String, row: Option<&SystemRow>) -> Shown {
    let status = status_of(feed.detail(&data_request(&project)));
    let served = data_of(feed, &project);
    let scene = served.map(|served| data::build(served, &state.filter));
    let total = served.map_or(0, |served| {
        served
            .rows
            .iter()
            .filter(|row| matches!(row, SystemDataRow::Type { .. }))
            .count()
    });
    let filtering = !state.filter.trim().is_empty();
    let empty = match (&scene, served) {
        (Some(scene), Some(_)) if scene.cards.is_empty() && total > 0 && filtering => {
            Some(data::FILTERED)
        }
        (Some(scene), Some(served)) if scene.cards.is_empty() => data::said(data::outcome(served)),
        _ => None,
    };
    let matched = scene.as_ref().filter(|_| filtering).map(|scene| {
        let drawn = scene.cards.iter().filter(|c| c.data.is_some()).count();
        let more = scene
            .cards
            .iter()
            .filter_map(|c| c.more)
            .map(|m| m.0 as usize)
            .sum::<usize>();
        (drawn + more, total)
    });
    Shown {
        projects: Vec::new(),
        status,
        crumbs: vec![project.clone()],
        notices: {
            let mut notices = data::notices(served, row);
            if let Some(n) = scene.as_ref().map(|s| s.undrawn.len()).filter(|n| *n > 0) {
                notices.push(data::undrawn_notice(n));
            }
            notices
        },
        summary: served.map(data::summary).unwrap_or_default(),
        legend: data::legend(),
        inspector: scene
            .as_ref()
            .zip(state.picked.as_ref())
            .and_then(|(scene, pick)| inspect(scene, pick, None)),
        matched,
        provenance: data::provenance(served, row),
        empty,
        scene,
        project: Some(project),
        hint: DATA_HINT,
    }
}

/// The Runtime view: one level, the units and what starts them.
fn runtime_shown(state: &State, feed: &Feed, project: String, row: Option<&SystemRow>) -> Shown {
    let status = status_of(feed.detail(&runtime_request(&project)));
    let served = runtime_of(feed, &project);
    let scene = served.map(|runtime| runtime::build(runtime, state.scripts));
    let empty = match (&scene, served) {
        (Some(scene), Some(runtime)) if scene.cards.is_empty() && runtime.error.is_none() => {
            Some(runtime::EMPTY)
        }
        _ => None,
    };
    Shown {
        projects: Vec::new(),
        status,
        crumbs: vec![project.clone()],
        notices: runtime::notices(served, row),
        summary: served.map(runtime::summary).unwrap_or_default(),
        legend: scene.as_ref().map(runtime::legend).unwrap_or_default(),
        inspector: scene
            .as_ref()
            .zip(state.picked.as_ref())
            .and_then(|(scene, pick)| inspect(scene, pick, None)),
        matched: scene
            .as_ref()
            .filter(|_| !state.filter.trim().is_empty())
            .map(|scene| matched(scene, &state.filter)),
        provenance: runtime::provenance(served, row),
        empty,
        scene,
        project: Some(project),
        hint: RUNTIME_HINT,
    }
}

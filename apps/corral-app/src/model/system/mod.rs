//! Activity › System: one project's map. The page reads the project's `system` row (whether
//! its facts are building, its extension and that extension's last error) and the
//! `system_map` detail of the level shown, nothing else. Every form shares this state: the
//! trail of levels drilled into, what is picked (a node or an edge), the overlay, the activity
//! window and the filter. `scene` turns a served level into cards and edges, `layout` places
//! them for the canvas, `tree` lists them for the phone and the terminal, `inspect` says what
//! the pick is, and `words` the page's sentences. The Runtime view reads the `system_runtime`
//! detail instead and `runtime` turns it into a scene the same code places, lists and inspects.
//! The Lenses view reads `system_lenses` and `system_lens`: `lens` turns a checked lens into a
//! diagram and `lens_layout` places it. The Data view reads `system_data` and `data` turns it
//! into a scene of types by container, as Runtime does.

pub mod data;
pub mod data_inspect;
pub mod inspect;
mod layered;
pub mod layout;
pub mod lens;
pub mod lens_layout;
pub mod marks;
pub mod runtime;
mod runtime_inspect;
pub mod scene;
pub mod shown;
pub mod tree;
pub mod words;

use corral_protocol::detail::DetailRef;
use corral_types::lenses::LensKind;
use corral_types::{DataTypeId, ModuleId, RuntimeUnitId, SystemAt};

use crate::model::Id;

/// Which map the page draws: what the project is made of, what it runs, or how it behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Structure,
    Runtime,
    /// The types it stores and sends, and which types hold which.
    Data,
    /// The project's saved state and sequence diagrams.
    Lenses,
}

impl View {
    pub const ALL: [View; 4] = [Self::Structure, Self::Runtime, Self::Data, Self::Lenses];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Structure => "Structure",
            Self::Runtime => "Runtime",
            Self::Data => "Data",
            Self::Lenses => "Lenses",
        }
    }
    /// The address's last segment.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Runtime => "runtime",
            Self::Data => "data",
            Self::Lenses => "lenses",
        }
    }
    pub fn from_slug(slug: &str) -> Option<View> {
        Self::ALL.into_iter().find(|v| v.slug() == slug)
    }
}

/// The Draft a lens form: the kind and the owner's one sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub kind: LensKind,
    pub about: String,
}

/// A task this page filed (a draft or a repair): the operation's key, read for its outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Filed {
    pub key: Id,
    pub title: String,
}

/// What re-colours the same geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlay {
    /// Lines, language mix and workspace status.
    #[default]
    Structure,
    /// Commits and lines changed in the window.
    Activity,
    /// Files read versus not.
    Coverage,
}

impl Overlay {
    pub const ALL: [Overlay; 3] = [Self::Structure, Self::Activity, Self::Coverage];
    pub const fn label(self) -> &'static str {
        match self {
            // "Structure" names the view this overlay re-colours.
            Self::Structure => "Size",
            Self::Activity => "Activity",
            Self::Coverage => "Coverage",
        }
    }
}

/// The activity overlay's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Window {
    Days7,
    #[default]
    Days30,
}

impl Window {
    pub const ALL: [Window; 2] = [Self::Days7, Self::Days30];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Days7 => "7 days",
            Self::Days30 => "30 days",
        }
    }
    pub const fn days(self) -> u32 {
        match self {
            Self::Days7 => 7,
            Self::Days30 => 30,
        }
    }
}

/// A node on the map at some level.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Node {
    /// A package or a top-level folder, by repository-relative path ("" for the root).
    Container(String),
    /// A neighbouring package on a package's boundary: above it when it references the package
    /// shown (`into`), below it when the package shown references it.
    Port {
        path: String,
        into: bool,
    },
    Module(ModuleId),
    File(String),
    Definition {
        line: u64,
        name: String,
    },
    /// What a level does not list, counted.
    More,
    /// A runtime unit: a binary, script, service, image, systemd unit or CI job.
    Unit(RuntimeUnitId),
    /// The project's npm scripts, folded into one card.
    Scripts,
    /// A data type on the Data view.
    Type(DataTypeId),
}

impl Node {
    /// The level selecting this node again opens, if it has one.
    pub fn opens(&self) -> Option<SystemAt> {
        match self {
            Node::Container(path) | Node::Port { path, .. } => {
                Some(SystemAt::Container { path: path.clone() })
            }
            Node::Module(module) => Some(SystemAt::Module {
                module: module.clone(),
            }),
            Node::File(path) => Some(SystemAt::File { path: path.clone() }),
            Node::Definition { .. }
            | Node::More
            | Node::Unit(_)
            | Node::Scripts
            | Node::Type(_) => None,
        }
    }

    /// The node a level is opened from, which going up selects.
    pub fn of(at: &SystemAt) -> Node {
        match at {
            SystemAt::Container { path } => Node::Container(path.clone()),
            SystemAt::Module { module } => Node::Module(module.clone()),
            SystemAt::File { path } => Node::File(path.clone()),
        }
    }

    /// A stable id for tests and focus: `node-<kind>-<path>`.
    pub fn test_id(&self) -> String {
        match self {
            Node::Container(path) => format!("node-c-{}", slug(path)),
            Node::Port { path, into } => {
                format!("node-p{}-{}", if *into { "in" } else { "out" }, slug(path))
            }
            Node::Module(module) => {
                format!("node-m-{}-{}", slug(&module.container), slug(&module.path))
            }
            Node::File(path) => format!("node-f-{}", slug(path)),
            Node::Definition { line, .. } => format!("node-d-{line}"),
            Node::More => "node-more".into(),
            Node::Unit(id) => format!(
                "node-u-{}-{}-{}",
                runtime::kind_slug(id.kind),
                slug(&id.path),
                slug(&id.name)
            ),
            Node::Scripts => "node-scripts".into(),
            Node::Type(id) => format!("node-t-{}-{}", slug(&id.path), slug(&id.name)),
        }
    }
}

fn slug(path: &str) -> String {
    if path.is_empty() {
        return "root".into();
    }
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// What the inspector shows: a node or an edge between two nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Node(Node),
    Edge(Node, Node),
}

/// The page's state on this device.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub view: View,
    /// The Runtime view lists every npm script instead of one folded card.
    pub scripts: bool,
    /// The project the trail belongs to; another scope starts at its project level.
    pub project: Option<String>,
    /// The levels drilled into, outermost first; empty at the project level.
    pub trail: Vec<SystemAt>,
    pub picked: Option<Pick>,
    pub overlay: Overlay,
    pub window: Window,
    pub filter: String,
    /// The lens drawn: its id, or the file of one without an id.
    pub lens: Option<String>,
    /// The lens element picked, by the checker's name for it.
    pub element: Option<String>,
    /// The Draft a lens form, while open.
    pub draft: Option<Draft>,
    pub filed: Option<Filed>,
}

/// What the owner does on the page.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Selects a node or an edge; selecting the picked node again opens it.
    Pick(Pick),
    /// Opens a node's level.
    Open(Node),
    /// Goes to the breadcrumb at `depth` (0 the project).
    To(usize),
    /// One level up, selecting the node it was opened from; at the project level, unpicks.
    Up,
    Overlay(Overlay),
    Window(Window),
    Filter(String),
    View(View),
    /// Draws another lens.
    Lens(String),
    /// Picks a lens element; picking it again lets go.
    Element(String),
    /// Opens, edits or closes (none) the Draft a lens form.
    Draft(Option<Draft>),
    Filed(Filed),
}

impl State {
    /// The level shown.
    pub fn at(&self) -> Option<&SystemAt> {
        self.trail.last()
    }

    /// Starts over at the project level when the scope names another project.
    pub fn follow(&mut self, project: &str) {
        if self.project.as_deref() != Some(project) {
            self.project = Some(project.to_owned());
            self.trail.clear();
            self.picked = None;
            self.filter.clear();
            self.lens = None;
            self.element = None;
            self.draft = None;
            self.filed = None;
        }
    }

    /// Applies an action; whether anything changed.
    pub fn act(&mut self, action: Action) -> bool {
        let before = self.clone();
        match action {
            Action::Pick(pick) => {
                if self.picked.as_ref() == Some(&pick) {
                    if let Pick::Node(node) = pick {
                        self.open(&node);
                    }
                } else {
                    self.picked = Some(pick);
                }
            }
            Action::Open(node) => self.open(&node),
            // The Runtime and Lenses views have one level: the trail is the Structure view's.
            Action::To(_) if self.view != View::Structure => {}
            Action::Up if matches!(self.view, View::Runtime | View::Data) => self.picked = None,
            Action::Up if self.view == View::Lenses => {
                if self.draft.is_some() {
                    self.draft = None;
                } else {
                    self.element = None;
                }
            }
            Action::To(depth) => {
                if depth < self.trail.len() {
                    let left = self.trail.get(depth).map(Node::of);
                    self.trail.truncate(depth);
                    self.picked = left.map(Pick::Node);
                }
            }
            Action::Up => match self.trail.pop() {
                Some(at) => self.picked = Some(Pick::Node(Node::of(&at))),
                None => self.picked = None,
            },
            Action::Overlay(overlay) => self.overlay = overlay,
            Action::Window(window) => self.window = window,
            Action::Filter(text) => self.filter = text,
            Action::View(view) => {
                if self.view != view {
                    self.view = view;
                    self.picked = None;
                }
            }
            Action::Lens(lens) => {
                if self.lens.as_ref() != Some(&lens) {
                    self.lens = Some(lens);
                    self.element = None;
                }
            }
            Action::Element(name) => {
                self.element = (self.element.as_ref() != Some(&name)).then_some(name);
            }
            Action::Draft(draft) => self.draft = draft,
            Action::Filed(filed) => {
                self.draft = None;
                self.filed = Some(filed);
            }
        }
        *self != before
    }

    fn open(&mut self, node: &Node) {
        if self.view == View::Runtime {
            // Selecting the folded scripts again unfolds them; a script's inspector folds them.
            if *node == Node::Scripts {
                self.scripts = !self.scripts;
                self.picked = (!self.scripts).then_some(Pick::Node(Node::Scripts));
            }
            return;
        }
        let Some(at) = node.opens() else {
            return;
        };
        // A neighbour on a package's boundary is another package: its own level replaces this.
        if matches!(node, Node::Port { .. }) {
            self.trail.clear();
        }
        self.trail.push(at);
        self.picked = None;
    }
}

/// The `system_map` detail of the level shown.
pub fn request(project: &str, at: Option<&SystemAt>) -> DetailRef {
    DetailRef::SystemMap {
        project: project.to_owned(),
        at: at.cloned(),
    }
}

/// The `system_runtime` detail: the project's units, edges and entry points.
pub fn runtime_request(project: &str) -> DetailRef {
    DetailRef::SystemRuntime {
        project: project.to_owned(),
    }
}

/// The `system_data` detail at the project head: types, has edges, containers and coverage.
pub fn data_request(project: &str) -> DetailRef {
    DetailRef::SystemData {
        project: project.to_owned(),
        at: None,
    }
}

/// The detail the page's view reads at the level shown.
pub fn view_request(state: &State, project: &str, at: Option<&SystemAt>) -> DetailRef {
    match state.view {
        View::Structure => request(project, at),
        View::Runtime => runtime_request(project),
        View::Data => data_request(project),
        View::Lenses => lens::list_request(project),
    }
}

/// `1,234`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 file`, `3 files`.
pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// The last segment of a path.
pub fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests;

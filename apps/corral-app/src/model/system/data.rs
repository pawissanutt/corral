//! The Data view (docs/plans/system-map.md, data model): the served `system_data` as a scene
//! the System map's own layout, canvas, tree and inspector draw. Cards are the data types (JSON
//! Schema definitions, SQL tables, Rust types that derive `Serialize` or `Deserialize`, and an
//! extension's type rows), each in its container's band; edges are `has`, one per pair of
//! types with every field that names the other. The filter narrows the types by name or path
//! before the scene is built, so a large project stays legible; past `MAX_TYPES` the rest are
//! counted in one card.

use std::collections::{BTreeMap, BTreeSet};

use corral_types::{DataSource, DataTypeId, SystemData, SystemDataRow, SystemRow, SystemStatus};

use super::scene::{
    Card, DataCard, Edge, EdgeKind, Kind, Level, Scene, data_kind_word, keep_legible,
};
use super::tree::matches;
use super::words::{Notice, Swatch};
use super::{Node, count, plural};
use crate::model::Hue;

/// Types the view draws before counting the rest.
pub const MAX_TYPES: usize = 240;
/// `has` edges the map draws beyond each type's strongest; the rest stay listed in the tree and
/// the inspector.
pub const MAX_DRAWN_EDGES: usize = 80;

pub const fn source_word(source: DataSource) -> &'static str {
    match source {
        DataSource::JsonSchema => "JSON Schema",
        DataSource::Sql => "SQL",
        DataSource::RustSerde => "serialized Rust type",
        DataSource::Extension => "extension",
    }
}

/// How the type was read: a schema file declares it, a serde derive is syntax, an extension's
/// row is resolved.
pub const fn basis_word(source: DataSource) -> &'static str {
    match source {
        DataSource::JsonSchema | DataSource::Sql => "declared",
        DataSource::RustSerde => "syntax",
        DataSource::Extension => "resolved",
    }
}

/// The mark a type from a schema file or a serialized type carries; none for an extension's.
pub const fn mark(source: DataSource) -> Option<&'static str> {
    match source {
        DataSource::JsonSchema | DataSource::Sql => Some("schema"),
        DataSource::RustSerde => Some("serialized"),
        DataSource::Extension => None,
    }
}

/// A field type as read: corrald serves Rust types as spaced tokens (`Option < String >`), so a
/// reformat never reads as a retype; drawn without the spaces around `<`, `>`, `::`, `&` and
/// before `,`.
pub fn type_words(ty: &str) -> String {
    let mut out = ty.to_owned();
    for (from, to) in [
        (" < ", "<"),
        (" <", "<"),
        ("< ", "<"),
        (" >", ">"),
        (" ,", ","),
        (" :: ", "::"),
        ("& ", "&"),
    ] {
        out = out.replace(from, to);
    }
    out
}

/// A container's path in a sentence.
pub fn container_words(path: &str) -> String {
    if path.is_empty() {
        "(root)".into()
    } else {
        path.to_owned()
    }
}

/// A package or folder as `system_data` names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub path: String,
    pub name: String,
    pub languages: BTreeSet<String>,
    /// The sources that read its files, as corrald says.
    pub sources: BTreeSet<DataSource>,
}

pub fn containers(data: &SystemData) -> Vec<Container> {
    let mut out: Vec<Container> = data
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemDataRow::Container {
                path,
                name,
                languages,
                sources,
            } => Some(Container {
                path: path.clone(),
                name: name.clone(),
                languages: languages.clone(),
                sources: sources.clone(),
            }),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The deepest container holding `path`; without containers (an older corrald), the folder
/// above its `src`, or its own folder.
pub fn holder(containers: &[Container], path: &str) -> String {
    if containers.is_empty() {
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        return match dir.split_once("/src") {
            Some((above, _)) => above.to_owned(),
            None if dir == "src" => String::new(),
            None => dir.to_owned(),
        };
    }
    containers
        .iter()
        .map(|c| c.path.as_str())
        .filter(|c| c.is_empty() || path == *c || path.starts_with(&format!("{c}/")))
        .max_by_key(|c| c.len())
        .unwrap_or_default()
        .to_owned()
}

/// The languages corrald observed and the data sources that supplied each; an empty set is
/// "not provided", never zero.
pub fn coverage(data: &SystemData) -> BTreeMap<&str, &BTreeSet<DataSource>> {
    data.rows
        .iter()
        .filter_map(|row| match row {
            SystemDataRow::Coverage { language, sources } => Some((language.as_str(), sources)),
            _ => None,
        })
        .collect()
}

/// Whether some source read the container's own files: a container none did has its data model
/// not provided rather than empty. Another container's schema in the same language says nothing
/// about this one.
pub fn container_provided(container: &Container) -> bool {
    !container.sources.is_empty()
}

fn card_tag(source: DataSource, kind: corral_types::DataKind) -> String {
    match mark(source) {
        Some(mark) => format!("{} · {mark}", data_kind_word(kind)),
        None => data_kind_word(kind).to_owned(),
    }
}

/// The view's scene: the types the filter matches, in their containers' bands.
pub fn build(data: &SystemData, filter: &str) -> Scene {
    let mut scene = Scene {
        level: Level::Data,
        cards: Vec::new(),
        graph: Vec::new(),
        top: Vec::new(),
        bottom: Vec::new(),
        band: Vec::new(),
        band_title: None,
        ranks: None,
        rank_titles: BTreeMap::new(),
        edges: Vec::new(),
        undrawn: Vec::new(),
        max_lines: 1,
    };
    let known = containers(data);
    let mut holds: BTreeMap<DataTypeId, Vec<(DataTypeId, String, u64)>> = BTreeMap::new();
    let mut held_by: BTreeMap<DataTypeId, Vec<(DataTypeId, String, u64)>> = BTreeMap::new();
    for row in &data.rows {
        if let SystemDataRow::Has { edge } = row {
            holds.entry(edge.from.clone()).or_default().push((
                edge.to.clone(),
                edge.field.clone(),
                edge.line,
            ));
            held_by.entry(edge.to.clone()).or_default().push((
                edge.from.clone(),
                edge.field.clone(),
                edge.line,
            ));
        }
    }
    let mut cards: Vec<Card> = Vec::new();
    for row in &data.rows {
        let SystemDataRow::Type { data_type: ty } = row else {
            continue;
        };
        let id = ty.id();
        let mut card = Card::new(Node::Type(id.clone()), ty.name.clone(), Kind::Type(ty.kind));
        card.tag = Some(card_tag(ty.source, ty.kind));
        card.lines = ty.fields.len() as u64;
        card.data = Some(DataCard {
            ty: ty.clone(),
            container: holder(&known, &ty.path),
            holds: holds.remove(&id).unwrap_or_default(),
            held_by: held_by.remove(&id).unwrap_or_default(),
        });
        if matches(&card, filter) {
            cards.push(card);
        }
    }
    let container_of = |card: &Card| card.data.as_ref().map(|d| d.container.clone());
    cards.sort_by(|a, b| (container_of(a), &a.name).cmp(&(container_of(b), &b.name)));
    let rest = cards.len().saturating_sub(MAX_TYPES);
    cards.truncate(MAX_TYPES);
    let shown: BTreeSet<String> = cards.iter().filter_map(container_of).collect();
    // The first container in path order sits at the top.
    let rank: BTreeMap<&String, i32> = shown
        .iter()
        .enumerate()
        .map(|(i, path)| (path, (shown.len() - 1 - i) as i32))
        .collect();
    let mut ranks = Vec::new();
    for card in &cards {
        let path = container_of(card).unwrap_or_default();
        let at = rank.get(&path).copied().unwrap_or(0);
        ranks.push(at);
        scene.rank_titles.entry(at).or_insert_with(|| {
            match known.iter().find(|c| c.path == path) {
                Some(c) if !c.name.is_empty() && !path.ends_with(&c.name) => {
                    format!("{} ({})", container_words(&path), c.name)
                }
                _ => container_words(&path),
            }
        });
    }
    let nodes: BTreeSet<Node> = cards.iter().map(|c| c.node.clone()).collect();
    let mut all = Vec::new();
    for card in &cards {
        let Some(data) = &card.data else { continue };
        let mut pairs: BTreeMap<&DataTypeId, u64> = BTreeMap::new();
        for (to, _, _) in &data.holds {
            *pairs.entry(to).or_default() += 1;
        }
        for (to, fields) in pairs {
            let to = Node::Type(to.clone());
            if to == card.node || !nodes.contains(&to) {
                continue;
            }
            all.push(Edge {
                from: card.node.clone(),
                to,
                kind: EdgeKind::Has,
                count: fields,
                basis: None,
                pairs: Vec::new(),
                sites: Vec::new(),
                change: Vec::new(),
            });
        }
    }
    scene.graph = (0..cards.len()).collect();
    scene.cards = cards;
    keep_legible(&mut scene, all, MAX_DRAWN_EDGES);
    scene.ranks = Some(ranks);
    if rest > 0 {
        let mut card = Card::new(
            Node::More,
            format!("{} more types", count(rest as u64)),
            Kind::More,
        );
        card.more = Some((rest as u64, "types"));
        scene.band.push(scene.cards.len());
        scene.cards.push(card);
        if let Some(ranks) = scene.ranks.as_mut() {
            ranks.push(0);
        }
    }
    scene.max_lines = scene
        .cards
        .iter()
        .map(|c| c.lines)
        .max()
        .unwrap_or(0)
        .max(1);
    scene
}

/// What a type's card says under its name: its first fields.
pub fn figure(card: &Card) -> String {
    let Some(data) = &card.data else {
        return String::new();
    };
    let fields = &data.ty.fields;
    if fields.is_empty() {
        return match data.ty.kind {
            corral_types::DataKind::Enum => "no variants read".into(),
            _ => "no fields read".into(),
        };
    }
    let names: Vec<&str> = fields.iter().take(3).map(|f| f.name.as_str()).collect();
    let mut words = names.join(", ");
    if fields.len() > 3 {
        words.push_str(&format!(" +{}", fields.len() - 3));
    }
    words
}

/// The figures over the map: types by source, `has` edges, and what is not provided.
pub fn summary(data: &SystemData) -> Vec<String> {
    let mut by_source: BTreeMap<DataSource, u64> = BTreeMap::new();
    let (mut fields, mut edges) = (0u64, 0u64);
    for row in &data.rows {
        match row {
            SystemDataRow::Type { data_type } => {
                *by_source.entry(data_type.source).or_default() += 1;
                fields += data_type.fields.len() as u64;
            }
            SystemDataRow::Has { .. } => edges += 1,
            _ => {}
        }
    }
    let total: u64 = by_source.values().sum();
    let mut out = Vec::new();
    if total == 0 {
        out.push(
            match outcome(data) {
                Outcome::Pending => "Data facts not read yet",
                Outcome::NotProvided => "Data model not provided",
                Outcome::Incomplete => "No data types read; the answer is incomplete",
                Outcome::Empty | Outcome::Types => "No data types",
            }
            .into(),
        );
    } else {
        let sources: Vec<String> = by_source
            .iter()
            .map(|(source, n)| {
                let words = match source {
                    DataSource::JsonSchema => "from JSON Schema",
                    DataSource::Sql => "from SQL",
                    DataSource::RustSerde => "serialized Rust",
                    DataSource::Extension => "from an extension",
                };
                format!("{} {words}", count(*n))
            })
            .collect();
        out.push(format!(
            "{} ({}) · {} · {}",
            plural(total, "type", "types"),
            sources.join(", "),
            plural(fields, "field", "fields"),
            plural(edges, "has edge", "has edges")
        ));
    }
    let coverage = coverage(data);
    let missing: Vec<&str> = coverage
        .iter()
        .filter(|(_, sources)| sources.is_empty())
        .map(|(language, _)| *language)
        .collect();
    if !missing.is_empty() {
        out.push(format!("Not provided for {}", missing.join(", ")));
    }
    let unprovided: Vec<String> = containers(data)
        .iter()
        .filter(|c| !c.languages.is_empty() && !container_provided(c))
        .map(|c| container_words(&c.path))
        .collect();
    if !unprovided.is_empty() {
        out.push(format!(
            "Not provided in {}: {}",
            plural(unprovided.len() as u64, "container", "containers"),
            unprovided.join(", ")
        ));
    }
    out
}

/// The edge kind the map draws; a card's `schema` or `serialized` mark explains itself in the
/// inspector.
pub fn legend() -> Vec<(Swatch, String)> {
    vec![(
        Swatch::Edge(EdgeKind::Has),
        "has: a field's type names the other type".to_owned(),
    )]
}

/// What the view says over the map: facts building, a failed read, files not read, rows left
/// out.
pub fn notices(data: Option<&SystemData>, row: Option<&SystemRow>) -> Vec<Notice> {
    let mut out = Vec::new();
    let building = data.map(|d| d.status) == Some(SystemStatus::Building)
        || row.map(|r| r.status) == Some(SystemStatus::Building);
    if building {
        out.push(Notice {
            hue: Hue::Corral,
            words: "Building this project's facts. The view fills in when they are ready.".into(),
        });
    }
    if let Some(error) = data
        .and_then(|d| d.error.clone())
        .or_else(|| row.and_then(|r| r.error.clone()))
    {
        out.push(Notice {
            hue: Hue::Fail,
            words: format!("The data facts could not be built: {error}"),
        });
    }
    if let Some(error) = data
        .and_then(|d| d.provider.as_ref())
        .and_then(|p| p.error.as_ref().map(|e| (p.extension.clone(), e.clone())))
    {
        out.push(Notice {
            hue: Hue::Held,
            words: format!(
                "{} failed, so its type rows are not shown: {}",
                error.0, error.1
            ),
        });
    }
    let Some(data) = data else {
        return out;
    };
    let unreadable: Vec<&str> = data
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemDataRow::Unreadable { path } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    if let Some(first) = unreadable.first() {
        out.push(Notice {
            hue: Hue::Neutral,
            words: format!(
                "{} could not be read, so their types are not shown (first: {first}).",
                plural(unreadable.len() as u64, "file", "files")
            ),
        });
    }
    if data.omitted_rows > 0 {
        out.push(Notice {
            hue: Hue::Neutral,
            words: format!(
                "{} left out to stay within the wire budget.",
                plural(data.omitted_rows as u64, "row", "rows")
            ),
        });
    }
    out
}

/// Says how many `has` edges the map leaves undrawn to stay legible.
pub fn undrawn_notice(n: usize) -> Notice {
    Notice {
        hue: Hue::Neutral,
        words: format!(
            "{} left undrawn on the map to keep it legible; each type's inspector and the list \
             name them all.",
            plural(n as u64, "has edge", "has edges")
        ),
    }
}

/// The one provenance line, with what the view cannot see.
pub fn provenance(data: Option<&SystemData>, row: Option<&SystemRow>) -> String {
    let head = data
        .and_then(|d| d.commit.clone())
        .or_else(|| row.and_then(|r| r.head.clone()));
    let at = match &head {
        Some(head) => format!("Facts at {}", head.chars().take(9).collect::<String>()),
        None => "No commit read yet".into(),
    };
    let extension = data
        .and_then(|d| d.provider.as_ref())
        .filter(|p| p.error.is_none())
        .map(|p| {
            format!(
                "; {}'s type rows replace them for the files they cover",
                p.extension
            )
        })
        .unwrap_or_default();
    format!(
        "{at} · built in from JSON Schema files, SQL files and Rust types that derive Serialize \
         or Deserialize{extension} · a field's type is its source text; has edges link this \
         project's own types"
    )
}

/// What a served `system_data` says about the project's types as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Still building, or served without facts yet: nothing is known.
    Pending,
    /// No source read any of the project's files: the data model is not provided, not empty.
    NotProvided,
    /// No type was read, but files could not be read or rows were left out.
    Incomplete,
    /// A completed scan whose sources read the project's files and found no type.
    Empty,
    /// Types were read.
    Types,
}

pub fn outcome(data: &SystemData) -> Outcome {
    let mut types = false;
    let (mut covered, mut unreadable, mut rows) = (false, false, false);
    for row in &data.rows {
        rows = true;
        match row {
            SystemDataRow::Type { .. } => types = true,
            SystemDataRow::Coverage { sources, .. } => covered |= !sources.is_empty(),
            SystemDataRow::Unreadable { .. } => unreadable = true,
            _ => {}
        }
    }
    if types {
        Outcome::Types
    } else if data.status == SystemStatus::Building || data.error.is_some() || !rows {
        // A rowless reply carries no coverage: it cannot say the project has no types.
        Outcome::Pending
    } else if unreadable || data.omitted_rows > 0 {
        Outcome::Incomplete
    } else if covered {
        Outcome::Empty
    } else {
        Outcome::NotProvided
    }
}

/// Said in the map's place when nothing is drawn and no filter hides anything.
pub fn said(outcome: Outcome) -> Option<&'static str> {
    match outcome {
        Outcome::Pending | Outcome::Types => None,
        Outcome::NotProvided => Some(NOT_PROVIDED),
        Outcome::Incomplete => Some(INCOMPLETE),
        Outcome::Empty => Some(EMPTY),
    }
}

pub const EMPTY: &str = "No data types at this commit: the built-in sources read this project's \
                         files and found no JSON Schema definition, SQL table or Rust type that \
                         derives Serialize or Deserialize.";
pub const NOT_PROVIDED: &str = "Data model not provided: no source reads this project's \
                                languages. JSON Schema, SQL and serialized Rust are built in; an \
                                extension can supply others.";
pub const INCOMPLETE: &str = "No data types were read, but some files could not be read or rows \
                              were left out, so this does not show the project has none.";
pub const FILTERED: &str = "No data type matches the filter.";

//! The Data view's inspector: a type's fields with their types, its source and basis, where it
//! is declared, and the types it holds and that hold it, each of which picks that type; a `has`
//! edge's fields. A type the filter hides is still named, without a pick.

use corral_types::{DataField, DataTypeId};

use super::data::{basis_word, container_words, mark, source_word, type_words};
use super::inspect::{Inspector, Row, Section};
use super::runtime::at;
use super::scene::{Card, EdgeKind, Scene, data_kind_word};
use super::{Node, Pick};

fn fact(label: &str, value: String) -> Row {
    Row {
        label: label.into(),
        value,
        pick: None,
        swatch: None,
    }
}

fn section(title: Option<String>, rows: Vec<Row>, note: Option<String>) -> Section {
    Section { title, rows, note }
}

fn capital(words: &str) -> String {
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// `name` or `name?` when the source marks it optional.
pub fn field_name(field: &DataField) -> String {
    if field.optional {
        format!("{}?", field.name)
    } else {
        field.name.clone()
    }
}

pub(super) fn inspect(scene: &Scene, pick: &Pick) -> Option<Inspector> {
    match pick {
        Pick::Node(node) => scene
            .card(node)
            .and_then(|card| node_inspector(scene, card)),
        Pick::Edge(from, to) => edge_inspector(scene, from, to),
    }
}

/// A linked type's row: picks it where the scene draws it.
fn linked(scene: &Scene, other: &DataTypeId, field: &str, line: u64, path: &str) -> Row {
    let node = Node::Type(other.clone());
    let drawn = scene.card(&node).is_some();
    Row {
        label: other.name.clone(),
        value: format!("field {field} · {}", at(path, line)),
        pick: drawn.then_some(Pick::Node(node)),
        swatch: drawn.then_some(EdgeKind::Has),
    }
}

fn node_inspector(scene: &Scene, card: &Card) -> Option<Inspector> {
    let data = card.data.as_ref()?;
    let ty = &data.ty;
    let mut facts = vec![
        fact("Declared", at(&ty.path, ty.line)),
        fact("Container", container_words(&data.container)),
        fact("Source", source_word(ty.source).to_owned()),
        fact("Basis", basis_word(ty.source).to_owned()),
    ];
    if let Some(mark) = mark(ty.source) {
        facts.push(fact(
            "Contract",
            match mark {
                "schema" => "declared in a schema file".into(),
                _ => "serialized: derives Serialize or Deserialize".into(),
            },
        ));
    }
    let mut sections = vec![section(None, facts, None)];
    let holds = |id: &DataTypeId, field: &str| {
        data.holds
            .iter()
            .find(|(to, f, _)| f == field && to != id)
            .map(|(to, _, _)| Node::Type(to.clone()))
            .filter(|node| scene.card(node).is_some())
    };
    let fields: Vec<Row> = ty
        .fields
        .iter()
        .map(|field| {
            let to = holds(&ty.id(), &field.name);
            Row {
                label: field_name(field),
                value: if field.ty.is_empty() {
                    format!(":{}", field.line)
                } else {
                    format!("{} · :{}", type_words(&field.ty), field.line)
                },
                pick: to.map(|to| Pick::Edge(card.node.clone(), to)),
                swatch: None,
            }
        })
        .collect();
    let noun = match ty.kind {
        corral_types::DataKind::Enum => "Variants",
        corral_types::DataKind::Table => "Columns",
        _ => "Fields",
    };
    sections.push(section(
        Some(format!("{noun} · {}", fields.len())),
        fields,
        ty.fields
            .is_empty()
            .then(|| "None read from the source.".to_owned()),
    ));
    let held: Vec<Row> = data
        .held_by
        .iter()
        .map(|(from, field, line)| linked(scene, from, field, *line, &from.path))
        .collect();
    sections.push(section(
        Some(format!("Held by · {}", held.len())),
        held,
        data.held_by
            .is_empty()
            .then(|| "No other type of this project has a field of this type.".to_owned()),
    ));
    let holds: Vec<Row> = data
        .holds
        .iter()
        .map(|(to, field, line)| linked(scene, to, field, *line, &ty.path))
        .collect();
    if !holds.is_empty() {
        sections.push(section(
            Some(format!("Holds · {}", holds.len())),
            holds,
            None,
        ));
    }
    let kind = match mark(ty.source) {
        Some(mark) => format!("{} · {mark}", capital(data_kind_word(ty.kind))),
        None => format!(
            "{} · {}",
            capital(data_kind_word(ty.kind)),
            source_word(ty.source)
        ),
    };
    Some(Inspector {
        kind,
        title: ty.name.clone(),
        sections,
        open: None,
    })
}

fn edge_inspector(scene: &Scene, from: &Node, to: &Node) -> Option<Inspector> {
    let card = scene.card(from)?;
    let data = card.data.as_ref()?;
    let Node::Type(target) = to else {
        return None;
    };
    let fields: Vec<Row> = data
        .holds
        .iter()
        .filter(|(other, _, _)| other == target)
        .map(|(_, field, line)| fact(field, at(&data.ty.path, *line)))
        .collect();
    let ends = [from, to]
        .into_iter()
        .map(|node| Row {
            label: format!("Select {}", scene.name(node)),
            value: String::new(),
            pick: Some(Pick::Node(node.clone())),
            swatch: None,
        })
        .collect();
    Some(Inspector {
        kind: "Data edge · has".into(),
        title: format!("{} has {}", data.ty.name, target.name),
        sections: vec![
            section(
                Some(format!("Through · {}", fields.len())),
                fields,
                Some("A field whose type names the other type of this project.".into()),
            ),
            section(None, ends, None),
        ],
        open: None,
    })
}

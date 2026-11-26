//! Mermaid's entity-relationship diagrams, read onto the same canvas as a flowchart: each entity
//! a box (its attributes' names after its name), each relationship an arrow from the entity
//! written first to the second, labelled with its words and both ends' cardinality so the
//! drawing says what Mermaid's crow's feet say. Anything else is an error the view shows beside
//! the source.

use super::{Built, at, label, starts_with_word};

/// Statements that only style or describe the drawing.
const SKIP: &[&str] = &[
    "classdef",
    "class",
    "style",
    "direction",
    "acctitle",
    "accdescr",
];

/// What the statements read so far leave open: the entity whose attribute block is open, and
/// every entity's attributes' names, joined onto its label once the whole diagram is read (an
/// entity may have several blocks, and an alias may name it after them).
#[derive(Default)]
struct Reading {
    open: Option<usize>,
    attributes: Vec<(usize, Vec<String>)>,
}

/// Reads every statement after the `erDiagram` header into `built`.
pub(super) fn lines<'a>(
    built: &mut Built,
    lines: impl Iterator<Item = (usize, &'a str)>,
) -> Result<(), String> {
    let mut reading = Reading::default();
    let mut last = 0;
    for (number, line) in lines {
        last = number;
        line_of(built, line, &mut reading).map_err(|why| at(number, &why))?;
    }
    if reading.open.is_some() {
        return Err(at(last, "an entity's attribute block is not closed"));
    }
    for (node, names) in reading.attributes {
        let shown = &mut built.nodes[node];
        shown.label = format!("{}: {}", shown.label, names.join(", "));
        shown.detail = Some(names.join(", "));
    }
    Ok(())
}

fn line_of(built: &mut Built, line: &str, reading: &mut Reading) -> Result<(), String> {
    if let Some(node) = reading.open {
        if line == "}" {
            reading.open = None;
            return Ok(());
        }
        let name = attribute(line)?;
        match reading.attributes.iter_mut().find(|(at, _)| *at == node) {
            Some((_, names)) => names.push(name),
            None => reading.attributes.push((node, vec![name])),
        }
        return Ok(());
    }
    if line.split_whitespace().nth(1).is_some() && starts_with_word(line, SKIP) {
        return Ok(());
    }
    if let Some(head) = line.strip_suffix('{') {
        reading.open = Some(entity(built, head.trim())?);
        return Ok(());
    }
    if let Some(head) = line
        .strip_suffix('}')
        .and_then(|rest| rest.trim_end().strip_suffix('{'))
    {
        entity(built, head.trim())?;
        return Ok(());
    }
    if let Some(found) = connector(line) {
        return relationship(built, line, found);
    }
    entity(built, line).map(|_| ())
}

/// One attribute line, `type name [PK, FK, UK] ["comment"]`: its name. Anything else (a brace
/// that closes or opens a block mid-line, two attributes on one line) is refused, so the
/// drawing never says other than Mermaid's.
fn attribute(line: &str) -> Result<String, String> {
    let refused = || format!("the attribute “{line}” is not understood");
    let mut rest = line.trim();
    if let Some(open) = rest.strip_suffix('"') {
        let start = open.rfind('"').ok_or_else(refused)?;
        rest = open[..start].trim_end();
    }
    if rest.contains(['{', '}', '"']) {
        return Err(refused());
    }
    let mut words = rest.split_whitespace();
    let (Some(_), Some(name)) = (words.next(), words.next()) else {
        return Err(refused());
    };
    let keys = words.collect::<Vec<_>>().join(" ");
    let keys_ok = keys.is_empty()
        || keys
            .split(',')
            .all(|key| matches!(key.trim(), "PK" | "FK" | "UK"));
    if !keys_ok {
        return Err(refused());
    }
    Ok(name.to_owned())
}

/// `NAME`, `"Name with spaces"` or `id["Alias"]`: the entity's box.
fn entity(built: &mut Built, spelled: &str) -> Result<usize, String> {
    let (id, alias) = match spelled.split_once('[') {
        Some((id, rest)) => {
            let alias = rest
                .strip_suffix(']')
                .ok_or_else(|| format!("“{spelled}” is not understood"))?;
            (id.trim(), Some(label(alias)))
        }
        None => (spelled, None),
    };
    let quoted = id.starts_with('"') && id.ends_with('"') && id.len() >= 2;
    let name = label(id);
    if name.is_empty() || (!quoted && id.contains(|c: char| c.is_whitespace() || "{}:".contains(c)))
    {
        return Err(format!("“{spelled}” is not understood"));
    }
    built.node(&name, alias, "Entity")
}

/// One end's cardinality as written before the connector (`|o`, `||`, `}o`, `}|`).
fn left(mark: &str) -> Option<&'static str> {
    Some(match mark {
        "|o" => "zero or one",
        "||" => "exactly one",
        "}o" => "zero or more",
        "}|" => "one or more",
        _ => return None,
    })
}

/// One end's cardinality as written after the connector (`o|`, `||`, `o{`, `|{`).
fn right(mark: &str) -> Option<&'static str> {
    Some(match mark {
        "o|" => "zero or one",
        "||" => "exactly one",
        "o{" => "zero or more",
        "|{" => "one or more",
        _ => return None,
    })
}

/// Where a relationship's marks stand: the byte its left mark starts at, when the line has one
/// (`||--o{`, `}|..|{`).
fn connector(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    (2..bytes.len().saturating_sub(3)).find(|&at| {
        matches!(&bytes[at..at + 2], b"--" | b"..")
            && line.get(at - 2..at).and_then(left).is_some()
            && line.get(at + 2..at + 4).and_then(right).is_some()
    })
}

fn relationship(built: &mut Built, line: &str, found: usize) -> Result<(), String> {
    let (from_end, to_end) = (&line[found - 2..found], &line[found + 2..found + 4]);
    let identifying = &line[found..found + 2] == "--";
    let (to, words) = line[found + 4..]
        .split_once(':')
        .ok_or_else(|| format!("the relationship “{line}” has no colon"))?;
    let from = entity(built, line[..found - 2].trim())?;
    let to = entity(built, to.trim())?;
    let (Some(from_card), Some(to_card)) = (left(from_end), right(to_end)) else {
        return Err(format!("“{line}” is not understood"));
    };
    let words = label(words);
    let shown = format!("{words} ({from_card} to {to_card})");
    built.edge(from, to, shown.trim().to_owned(), "Relationship".into())?;
    if let Some(edge) = built.edges.last_mut() {
        edge.detail = Some(if identifying {
            "identifying".into()
        } else {
            "non-identifying".into()
        });
    }
    Ok(())
}

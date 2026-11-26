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

/// An entity whose attribute block is open: its box and the attributes' names so far.
type Open = Option<(usize, Vec<String>)>;

/// Reads every statement after the `erDiagram` header into `built`.
pub(super) fn lines<'a>(
    built: &mut Built,
    lines: impl Iterator<Item = (usize, &'a str)>,
) -> Result<(), String> {
    let mut open: Open = None;
    let mut last = 0;
    for (number, line) in lines {
        last = number;
        line_of(built, line, &mut open).map_err(|why| at(number, &why))?;
    }
    if open.is_some() {
        return Err(at(last, "an entity's attribute block is not closed"));
    }
    Ok(())
}

fn line_of(built: &mut Built, line: &str, open: &mut Open) -> Result<(), String> {
    if open.is_some() {
        if line == "}" {
            if let Some((node, names)) = open.take() {
                attributes(built, node, &names);
            }
            return Ok(());
        }
        // `type name [PK, FK] ["comment"]`: the name is what the box lists.
        let mut words = line.split_whitespace();
        let (Some(_), Some(name)) = (words.next(), words.next()) else {
            return Err(format!("the attribute “{line}” is not understood"));
        };
        if let Some((_, names)) = open.as_mut() {
            names.push(name.to_owned());
        }
        return Ok(());
    }
    if line.split_whitespace().nth(1).is_some() && starts_with_word(line, SKIP) {
        return Ok(());
    }
    if let Some(head) = line.strip_suffix('{') {
        *open = Some((entity(built, head.trim())?, Vec::new()));
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

/// The box's label with its attributes' names after the entity's own.
fn attributes(built: &mut Built, node: usize, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let shown = &mut built.nodes[node];
    shown.label = format!("{}: {}", shown.label, names.join(", "));
    shown.detail = Some(names.join(", "));
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

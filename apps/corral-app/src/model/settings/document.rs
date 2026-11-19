//! `corral.toml` as a document: each edit changes the one key it names in place, so comments,
//! key order, formatting and tables this app does not know survive a save. A field is named by
//! its path in the engine's own notation (`projects.site.gate`, `routing[0].profile`), the same
//! words a refused save uses to say where a problem is.

use std::fmt;

use serde_json::{Map, Value as Json};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// One step of a path: a table key, or a rule's place in an array of tables.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Seg {
    Key(String),
    Index(usize),
}

/// A key's place in the document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(pub Vec<Seg>);

impl Path {
    /// The path of `keys` under the root.
    pub fn of(keys: &[&str]) -> Self {
        Self(keys.iter().map(|key| Seg::Key((*key).to_owned())).collect())
    }

    /// The key `name` of the entry `entry` in the root table `root`: `projects.site.gate`.
    pub fn entry(root: &str, entry: &str, name: &str) -> Self {
        Self::of(&[root, entry, name])
    }

    /// The key `name` of routing rule `index`: `routing[0].profile`.
    pub fn rule(index: usize, name: &str) -> Self {
        Self(vec![
            Seg::Key("routing".into()),
            Seg::Index(index),
            Seg::Key(name.into()),
        ])
    }

    fn split(&self) -> Option<(&[Seg], &str)> {
        let (last, parent) = self.0.split_last()?;
        match last {
            Seg::Key(key) => Some((parent, key)),
            Seg::Index(_) => None,
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, seg) in self.0.iter().enumerate() {
            match seg {
                Seg::Key(key) if at == 0 => write!(f, "{key}")?,
                Seg::Key(key) => write!(f, ".{key}")?,
                Seg::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

pub fn parse(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>()
        .map_err(|error| error.to_string().trim().to_owned())
}

/// The table at `path`, following a rule's index into `[[routing]]` or an inline array.
fn table<'a>(at: &'a dyn TableLike, path: &[Seg]) -> Option<&'a dyn TableLike> {
    let Some((first, rest)) = path.split_first() else {
        return Some(at);
    };
    let Seg::Key(key) = first else {
        return None;
    };
    let item = at.get(key)?;
    if let Some((Seg::Index(index), rest)) = rest.split_first() {
        return table(element(item, *index)?, rest);
    }
    table(item.as_table_like()?, rest)
}

fn element(item: &Item, index: usize) -> Option<&dyn TableLike> {
    match item {
        Item::ArrayOfTables(rules) => rules.get(index).map(|rule| rule as &dyn TableLike),
        Item::Value(Value::Array(rules)) => rules
            .get(index)?
            .as_inline_table()
            .map(|rule| rule as &dyn TableLike),
        _ => None,
    }
}

/// The table at `path`, creating missing tables (implicit, so an empty one prints nothing)
/// when `create`.
fn table_mut<'a>(
    at: &'a mut dyn TableLike,
    path: &[Seg],
    create: bool,
) -> Option<&'a mut dyn TableLike> {
    let Some((first, rest)) = path.split_first() else {
        return Some(at);
    };
    let Seg::Key(key) = first else {
        return None;
    };
    if let Some((Seg::Index(index), rest)) = rest.split_first() {
        let rule = match at.get_mut(key)? {
            Item::ArrayOfTables(rules) => {
                rules.get_mut(*index).map(|rule| rule as &mut dyn TableLike)
            }
            Item::Value(Value::Array(rules)) => rules
                .get_mut(*index)?
                .as_inline_table_mut()
                .map(|rule| rule as &mut dyn TableLike),
            _ => None,
        }?;
        return table_mut(rule, rest, create);
    }
    if create && !at.contains_key(key) {
        let mut fresh = Table::new();
        fresh.set_implicit(true);
        at.insert(key, Item::Table(fresh));
    }
    table_mut(at.get_mut(key)?.as_table_like_mut()?, rest, create)
}

/// The item at `path`, if the document sets it.
pub fn get<'a>(doc: &'a DocumentMut, path: &Path) -> Option<&'a Item> {
    let (parent, key) = path.split()?;
    table(doc.as_table(), parent)?
        .get(key)
        .filter(|item| !item.is_none())
}

/// Sets `path` to `value`, keeping the comment and spacing around a value it replaces.
pub fn set(doc: &mut DocumentMut, path: &Path, value: Value) {
    let Some((parent, key)) = path.split() else {
        return;
    };
    let Some(parent) = table_mut(doc.as_table_mut(), parent, true) else {
        return;
    };
    match parent.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        None => {
            parent.insert(key, Item::Value(value));
        }
    }
}

/// Removes `path`, so the engine's inherited value applies.
pub fn remove(doc: &mut DocumentMut, path: &Path) {
    let Some((parent, key)) = path.split() else {
        return;
    };
    if let Some(parent) = table_mut(doc.as_table_mut(), parent, false) {
        parent.remove(key);
    }
}

/// A list key as lines: an array's strings, or one string as one line (a gate may be either).
pub fn lines(item: Option<&Item>) -> Vec<String> {
    match item.and_then(Item::as_value) {
        Some(Value::Array(items)) => items.iter().map(shown).collect(),
        Some(Value::String(line)) => vec![line.value().clone()],
        Some(other) => vec![shown(other)],
        None => Vec::new(),
    }
}

/// A value as the owner reads it: a string bare, anything else as TOML writes it.
pub fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.value().clone(),
        other => other.clone().decorated("", "").to_string(),
    }
}

/// One change to a list key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListEdit {
    Set(usize, String),
    Add,
    Remove(usize),
    /// Moves the line one place up (`true`) or down.
    Move(usize, bool),
}

/// Changes one line of the list at `path`, keeping the others' formatting. A single string
/// becomes a list of that line first; the last line removed leaves an empty list.
pub fn edit_list(doc: &mut DocumentMut, path: &Path, edit: &ListEdit) {
    let mut items = match get(doc, path).and_then(Item::as_value) {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(line)) => {
            let mut items = Array::new();
            items.push(line.value().as_str());
            items
        }
        _ => Array::new(),
    };
    match edit {
        ListEdit::Set(index, line) if *index < items.len() => {
            items.replace(*index, line.as_str());
        }
        ListEdit::Add => items.push(""),
        ListEdit::Remove(index) if *index < items.len() => {
            items.remove(*index);
        }
        ListEdit::Move(index, up) => {
            let target = if *up {
                index.checked_sub(1)
            } else {
                Some(index + 1)
            };
            if let Some(target) = target.filter(|target| *target < items.len())
                && *index < items.len()
            {
                let line = items.remove(*index);
                items.insert_formatted(target, line);
            }
        }
        ListEdit::Set(..) | ListEdit::Remove(_) => return,
    }
    set(doc, path, Value::Array(items));
}

/// A table of names to strings (a project's environment), in the document's order.
pub fn pairs(item: Option<&Item>) -> Vec<(String, String)> {
    item.and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .filter_map(|(key, item)| Some((key.to_owned(), shown(item.as_value()?))))
                .collect()
        })
        .unwrap_or_default()
}

/// Sets one name of the table at `path` (an inline table when the document has none yet), or
/// removes it when `value` is `None`.
pub fn set_pair(doc: &mut DocumentMut, path: &Path, name: &str, value: Option<&str>) {
    if get(doc, path).is_none() {
        if value.is_none() {
            return;
        }
        set(doc, path, Value::InlineTable(InlineTable::new()));
    }
    let Some(table) = table_mut(doc.as_table_mut(), &path.0, false) else {
        return;
    };
    match value {
        Some(value) => match table.get_mut(name).and_then(Item::as_value_mut) {
            Some(old) => {
                let decor = old.decor().clone();
                *old = Value::from(value);
                *old.decor_mut() = decor;
            }
            None => {
                table.insert(name, toml_edit::value(value));
            }
        },
        None => {
            table.remove(name);
        }
    }
}

/// The names under the root table `root` (projects, profiles, harnesses), in document order.
pub fn names(doc: &DocumentMut, root: &str) -> Vec<String> {
    doc.get(root)
        .and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .filter(|(_, item)| item.is_table_like())
                .map(|(key, _)| key.to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Adds an empty `[root.name]` table after the others; an existing name is left alone.
pub fn add_entry(doc: &mut DocumentMut, root: &str, name: &str) {
    let Some(parent) = table_mut(doc.as_table_mut(), &[Seg::Key(root.into())], true) else {
        return;
    };
    if !parent.contains_key(name) {
        parent.insert(name, Item::Table(Table::new()));
    }
}

pub fn remove_entry(doc: &mut DocumentMut, root: &str, name: &str) {
    if let Some(parent) = table_mut(doc.as_table_mut(), &[Seg::Key(root.into())], false) {
        parent.remove(name);
    }
}

/// How many routing rules the document has.
pub fn rule_count(doc: &DocumentMut) -> usize {
    match doc.get("routing") {
        Some(Item::ArrayOfTables(rules)) => rules.len(),
        Some(Item::Value(Value::Array(rules))) => rules.len(),
        _ => 0,
    }
}

/// One change to the routing rules, which match in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEdit {
    /// A rule after the others, naming `profile`.
    Add(String),
    Remove(usize),
    Move(usize, bool),
}

pub fn edit_rules(doc: &mut DocumentMut, edit: &RuleEdit) {
    if let RuleEdit::Add(profile) = edit {
        let mut rule = Table::new();
        rule.insert("profile", toml_edit::value(profile.as_str()));
        match doc.get_mut("routing") {
            Some(Item::ArrayOfTables(rules)) => rules.push(rule),
            Some(Item::Value(Value::Array(rules))) => {
                rules.push(Value::InlineTable(rule.into_inline_table()));
            }
            _ => {
                let mut rules = ArrayOfTables::new();
                rules.push(rule);
                doc.insert("routing", Item::ArrayOfTables(rules));
            }
        }
        return;
    }
    let count = rule_count(doc);
    let (from, to) = match edit {
        RuleEdit::Remove(index) => (*index, None),
        RuleEdit::Move(index, true) => (*index, index.checked_sub(1)),
        RuleEdit::Move(index, false) => (*index, Some(index + 1)),
        RuleEdit::Add(_) => return,
    };
    if from >= count || to.is_some_and(|to| to >= count) {
        return;
    }
    if matches!(edit, RuleEdit::Move(..)) && to.is_none() {
        return;
    }
    match doc.get_mut("routing") {
        Some(Item::ArrayOfTables(rules)) => {
            // Tables print in document position order, so the places stay with the indices.
            let places: Vec<Option<isize>> = rules.iter().map(Table::position).collect();
            let rule = rules.remove(from);
            if let Some(to) = to {
                rules.insert(to, rule);
            }
            for (rule, place) in rules.iter_mut().zip(places) {
                rule.set_position(place);
            }
        }
        Some(Item::Value(Value::Array(rules))) => {
            let rule = rules.remove(from);
            if let Some(to) = to {
                rules.insert_formatted(to, rule);
            }
        }
        _ => {}
    }
}

/// The document as JSON, as the engine's parsed configuration reads in the sample preview.
pub fn json(doc: &DocumentMut) -> Map<String, Json> {
    table_json(doc.as_table())
}

fn table_json(table: &dyn TableLike) -> Map<String, Json> {
    table
        .iter()
        .map(|(key, item)| (key.to_owned(), item_json(item)))
        .collect()
}

fn item_json(item: &Item) -> Json {
    match item {
        Item::None => Json::Null,
        Item::Value(value) => value_json(value),
        Item::Table(table) => Json::Object(table_json(table)),
        Item::ArrayOfTables(tables) => Json::Array(
            tables
                .iter()
                .map(|table| Json::Object(table_json(table)))
                .collect(),
        ),
    }
}

fn value_json(value: &Value) -> Json {
    match value {
        Value::String(text) => Json::String(text.value().clone()),
        Value::Integer(number) => Json::from(*number.value()),
        Value::Float(number) => Json::from(*number.value()),
        Value::Boolean(flag) => Json::Bool(*flag.value()),
        Value::Datetime(stamp) => Json::String(stamp.value().to_string()),
        Value::Array(items) => Json::Array(items.iter().map(value_json).collect()),
        Value::InlineTable(table) => Json::Object(table_json(table)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_comments_order_and_tables_the_app_does_not_know() {
        let text = "# workspace\n[defaults]\nprofile = \"a\" # worker\n\n[projects.app]\npath = \"app\"\ngate = [\"just check\"] # fast\n\n[[routing]]\nprofile = \"a\"\n\n[[routing]]\nprofile = \"b\"\n\n[future]\nkept = true\n";
        let mut doc = parse(text).unwrap();
        set(
            &mut doc,
            &Path::of(&["defaults", "profile"]),
            Value::from("b"),
        );
        edit_list(
            &mut doc,
            &Path::entry("projects", "app", "gate"),
            &ListEdit::Add,
        );
        edit_list(
            &mut doc,
            &Path::entry("projects", "app", "gate"),
            &ListEdit::Set(1, "just lint".into()),
        );
        edit_rules(&mut doc, &RuleEdit::Move(1, true));
        let out = doc.to_string();
        assert!(out.contains("profile = \"b\" # worker"), "{out}");
        assert!(
            out.contains("gate = [\"just check\", \"just lint\"] # fast"),
            "{out}"
        );
        assert!(
            out.contains("# workspace") && out.contains("[future]\nkept = true"),
            "{out}"
        );
        let rules = out.find("profile = \"b\"\n").unwrap() < out.find("profile = \"a\"\n").unwrap();
        assert!(rules, "the moved rule prints first: {out}");
    }
}

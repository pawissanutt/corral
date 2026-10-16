//! Comment-preserving edits of `.corral/corral.toml`.
//!
//! `ConfigDoc` wraps a `toml_edit` document so every change keeps the file's comments, key order and
//! formatting; `Config` stays the read model (`ConfigDoc::config`). Saves go through `Store::edit_config` /
//! `Store::save_config`, which validate before writing.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{DocumentMut, InlineTable, Item, Key, Table, TableLike, Value};

pub use toml_edit;

use crate::Config;

/// A parsed corral.toml that keeps comments, ordering and formatting through edits.
#[derive(Debug, Clone, Default)]
pub struct ConfigDoc {
    doc: DocumentMut,
}

impl fmt::Display for ConfigDoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.doc.fmt(f)
    }
}

impl ConfigDoc {
    /// Parses TOML text (syntax only; see [`ConfigDoc::config`] for the schema).
    pub fn parse(text: &str) -> Result<ConfigDoc> {
        let doc = text.parse::<DocumentMut>().context("invalid TOML")?;
        Ok(ConfigDoc { doc })
    }

    /// Reads and parses a corral.toml.
    pub fn load(path: &Path) -> Result<ConfigDoc> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        ConfigDoc::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// The document as the serde read model; errors when a value does not fit the schema.
    pub fn config(&self) -> Result<Config> {
        toml::from_str(&self.doc.to_string()).map_err(|e| anyhow!("{}", e.to_string().trim_end()))
    }

    /// The item at a dotted key (`limits.running`, `projects."my app".path`), if present.
    pub fn get(&self, key: &str) -> Result<Option<&Item>> {
        let path = parse_key(key)?;
        let mut cur = self.doc.as_item();
        for (i, seg) in path.iter().enumerate() {
            let Some(tbl) = cur.as_table_like() else {
                bail!("{} is not a table", dotted(&path[..i]));
            };
            match tbl.get(seg.get()) {
                Some(item) if !item.is_none() => cur = item,
                _ => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    /// Sets a dotted key to `value`, creating missing tables. An existing value keeps its surrounding
    /// comments and position; a table cannot be replaced by a value.
    pub fn set(&mut self, key: &str, mut value: Value) -> Result<()> {
        let path = parse_key(key)?;
        let (leaf, parents) = split_leaf(&path)?;
        let tbl = table_at(self.doc.as_item_mut(), parents, true)?
            .ok_or_else(|| anyhow!("cannot create {}", dotted(parents)))?;
        match tbl.get_mut(leaf.get()) {
            Some(Item::Value(old)) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            Some(item) if !item.is_none() => {
                bail!("{key} is a table; set its keys one by one")
            }
            _ => {
                value.decor_mut().clear();
                tbl.insert(leaf.get(), Item::Value(value));
            }
        }
        Ok(())
    }

    /// Sets a dotted key from command-line text: TOML syntax (`3`, `true`, `["a", "b"]`, `"quoted"`) is
    /// parsed as such, anything else is taken as a plain string.
    pub fn set_str(&mut self, key: &str, raw: &str) -> Result<()> {
        let value = raw
            .trim()
            .parse::<Value>()
            .unwrap_or_else(|_| Value::from(raw));
        self.set(key, value)
    }

    /// Removes a dotted key (value or whole table). Returns whether it existed.
    pub fn unset(&mut self, key: &str) -> Result<bool> {
        let path = parse_key(key)?;
        let (leaf, parents) = split_leaf(&path)?;
        let Some(tbl) = table_at(self.doc.as_item_mut(), parents, false)? else {
            return Ok(false);
        };
        Ok(tbl.remove(leaf.get()).is_some_and(|item| !item.is_none()))
    }

    /// Adds `[<table>.<name>]` (e.g. table "profiles", name "fast"). Errors when the entry already exists.
    pub fn add_entry(&mut self, table: &str, name: &str, mut entry: Table) -> Result<()> {
        let path = parse_key(table)?;
        let tbl = table_at(self.doc.as_item_mut(), &path, true)?
            .ok_or_else(|| anyhow!("cannot create {table}"))?;
        if tbl.get(name).is_some_and(|i| !i.is_none()) {
            bail!("{table}.{name} already exists");
        }
        entry.set_implicit(false);
        tbl.insert(name, Item::Table(entry));
        Ok(())
    }

    /// Removes `[<table>.<name>]`. Returns whether it existed.
    pub fn remove_entry(&mut self, table: &str, name: &str) -> Result<bool> {
        let path = parse_key(table)?;
        let Some(tbl) = table_at(self.doc.as_item_mut(), &path, false)? else {
            return Ok(false);
        };
        Ok(tbl.remove(name).is_some_and(|item| !item.is_none()))
    }

    /// Rewrites the document so it deserializes to `config`, changing only keys whose values differ: untouched
    /// keys keep their comments and formatting, keys `config` no longer has are removed, new ones are added.
    pub fn sync_from(&mut self, config: &Config) -> Result<()> {
        let new = config
            .to_toml()
            .parse::<DocumentMut>()
            .context("re-parsing the serialized config")?;
        merge_table(self.doc.as_table_mut(), new.as_table());
        Ok(())
    }

    /// Apply only the semantic difference between two effective snapshots to this raw layer.
    /// Inherited values and unrelated concurrent edits are never copied into the target file.
    pub fn apply_config_changes(&mut self, base: &Config, draft: &Config) -> Result<()> {
        let before = base.effective_value()?;
        let mut after = draft.effective_value()?;
        let before_table = before.as_table().context("config is not a table")?;
        let after_table = after.as_table_mut().context("config is not a table")?;
        for (key, value) in before_table {
            if value.is_table() && !after_table.contains_key(key) {
                // Removing the last known entry must not remove entries added since opening.
                after_table.insert(key.clone(), toml::Value::Table(toml::Table::new()));
            }
        }
        self.apply_delta("", Some(&before), Some(&after))
    }

    fn apply_delta(
        &mut self,
        key: &str,
        before: Option<&toml::Value>,
        after: Option<&toml::Value>,
    ) -> Result<()> {
        if before == after {
            return Ok(());
        }
        if let Some(toml::Value::Table(table)) = after {
            for (part, value) in table {
                let path = if key.is_empty() {
                    Key::new(part).to_string()
                } else {
                    format!("{key}.{}", Key::new(part))
                };
                self.apply_delta(&path, before.and_then(|v| v.get(part)), Some(value))?;
            }
            if let Some(toml::Value::Table(old)) = before {
                for part in old.keys().filter(|part| !table.contains_key(*part)) {
                    let path = if key.is_empty() {
                        Key::new(part).to_string()
                    } else {
                        format!("{key}.{}", Key::new(part))
                    };
                    self.unset(&path)?;
                }
            }
        } else if let Some(value) = after {
            self.set_str(key, &value.to_string())?;
        } else {
            self.unset(key)?;
        }
        Ok(())
    }
}

fn parse_key(key: &str) -> Result<Vec<Key>> {
    let path = Key::parse(key).with_context(|| format!("invalid key {key:?}"))?;
    if path.is_empty() {
        bail!("empty key");
    }
    Ok(path)
}

fn split_leaf(path: &[Key]) -> Result<(&Key, &[Key])> {
    path.split_last().ok_or_else(|| anyhow!("empty key"))
}

fn dotted(path: &[Key]) -> String {
    if path.is_empty() {
        return "the document root".into();
    }
    path.iter().map(Key::get).collect::<Vec<_>>().join(".")
}

/// Walks to the table at `path`; with `create`, missing tables are added (implicit, so an intermediate with
/// only sub-tables gets no header of its own). Errors when a segment is a value.
fn table_at<'a>(
    root: &'a mut Item,
    path: &[Key],
    create: bool,
) -> Result<Option<&'a mut dyn TableLike>> {
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        let inline = cur.is_inline_table();
        let Some(tbl) = cur.as_table_like_mut() else {
            bail!("{} is not a table", dotted(&path[..i]));
        };
        if tbl.get(seg.get()).is_none_or(Item::is_none) {
            if !create {
                return Ok(None);
            }
            let item = if inline {
                Item::Value(Value::InlineTable(InlineTable::new()))
            } else {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            };
            tbl.insert(seg.get(), item);
        }
        cur = tbl
            .get_mut(seg.get())
            .ok_or_else(|| anyhow!("could not create {}", dotted(&path[..=i])))?;
    }
    let path_name = dotted(path);
    cur.as_table_like_mut()
        .map(Some)
        .ok_or_else(|| anyhow!("{path_name} is not a table"))
}

fn merge_table(old: &mut dyn TableLike, new: &dyn TableLike) {
    let stale: Vec<String> = old
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| new.get(k).is_none())
        .collect();
    for k in stale {
        old.remove(&k);
    }
    for (k, item) in new.iter() {
        match old.get_mut(k) {
            Some(existing) if !existing.is_none() => merge_item(existing, item),
            _ => {
                old.insert(k, fresh(item));
            }
        }
    }
}

fn merge_item(old: &mut Item, new: &Item) {
    if let (Some(o), Some(n)) = (old.as_table_like_mut(), new.as_table_like()) {
        merge_table(o, n);
        return;
    }
    match (old, new) {
        (Item::Value(o), Item::Value(n)) => {
            if canonical(o).is_none_or(|c| Some(c) != canonical(n)) {
                let decor = o.decor().clone();
                *o = n.clone();
                *o.decor_mut() = decor;
            }
        }
        (old, new) => *old = fresh(new),
    }
}

/// A copy of `item` for insertion into another document: tables are rebuilt so they carry no position
/// from the source document (they land after their preceding sibling instead).
fn fresh(item: &Item) -> Item {
    match item {
        Item::Table(src) => {
            let mut t = Table::new();
            t.set_implicit(src.is_implicit());
            merge_table(&mut t, src);
            Item::Table(t)
        }
        other => other.clone(),
    }
}

/// Semantic value (formatting and comments ignored) for change detection.
fn canonical(v: &Value) -> Option<toml::Value> {
    let mut v = v.clone();
    v.decor_mut().clear();
    toml::from_str::<toml::Table>(&format!("v = {v}"))
        .ok()?
        .remove("v")
}

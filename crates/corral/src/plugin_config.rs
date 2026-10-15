use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::HerdrKeys;
pub use corral_herdr::config_path;
use toml_edit::{DocumentMut, Item, TableLike};

pub(super) fn read_config(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn entries(doc: &DocumentMut) -> Result<Vec<&dyn TableLike>> {
    let Some(keys) = doc.get("keys") else {
        return Ok(Vec::new());
    };
    let keys = keys.as_table_like().context("keys must be a TOML table")?;
    let Some(command) = keys.get("command") else {
        return Ok(Vec::new());
    };
    if let Some(tables) = command.as_array_of_tables() {
        return Ok(tables.iter().map(|table| table as &dyn TableLike).collect());
    }
    let array = command
        .as_array()
        .context("keys.command must be an array of tables")?;
    array
        .iter()
        .map(|value| {
            value
                .as_inline_table()
                .map(|table| table as &dyn TableLike)
                .context("keys.command entries must be tables")
        })
        .collect()
}

fn same_binding(actual: &dyn TableLike, expected: &dyn TableLike) -> bool {
    ["key", "type", "command"].iter().all(|field| {
        actual.get(field).and_then(Item::as_str) == expected.get(field).and_then(Item::as_str)
    })
}

/// Checks required keys and actions, not descriptions or formatting.
pub fn keybindings_present(keys: &HerdrKeys) -> Result<bool> {
    present_at(&config_path()?, keys)
}

fn present_at(path: &Path, keys: &HerdrKeys) -> Result<bool> {
    let text = read_config(path)?.unwrap_or_default();
    let doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;
    let (wanted, warnings) = resolved_bindings(&doc, path, keys)?;
    let actual = entries(&doc).with_context(|| format!("reading keys in {}", path.display()))?;
    let present = entries(&wanted)?.iter().all(|expected| {
        let key = expected.get("key").and_then(Item::as_str);
        let matching: Vec<_> = actual
            .iter()
            .filter(|entry| entry.get("key").and_then(Item::as_str) == key)
            .collect();
        !matching.is_empty()
            && matching
                .iter()
                .all(|entry| same_binding(**entry, *expected))
    });
    if !present && !warnings.is_empty() {
        bail!("{}", warnings.join("\n"));
    }
    Ok(present)
}

/// Selects only unoccupied keys, reserving every configured shortcut before proposing alternatives.
fn resolved_bindings(
    doc: &DocumentMut,
    path: &Path,
    keys: &HerdrKeys,
) -> Result<(DocumentMut, Vec<String>)> {
    let mut wanted = super::keybindings(keys).parse::<DocumentMut>()?;
    let actual = entries(doc)?;
    let mut reserved = std::collections::BTreeSet::new();
    for (action, key) in keys.bindings() {
        if key.trim().is_empty() || !reserved.insert(key.to_owned()) {
            bail!("herdr.keys.{action}: shortcuts must be nonempty and distinct");
        }
    }
    let mut occupied: std::collections::BTreeSet<_> = actual
        .iter()
        .filter_map(|entry| entry.get("key").and_then(Item::as_str))
        .map(str::to_owned)
        .collect();
    let native: std::collections::BTreeSet<_> = doc
        .get("keys")
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|table| table.iter())
        .filter_map(|(_, value)| value.as_str().map(str::to_owned))
        .collect();
    occupied.extend(native.iter().cloned());
    let mut warnings = Vec::new();
    for binding in wanted["keys"]["command"]
        .as_array_of_tables_mut()
        .context("corral keybindings must be tables")?
        .iter_mut()
    {
        let key = binding["key"]
            .as_str()
            .context("corral key is missing")?
            .to_owned();
        let command = binding["command"]
            .as_str()
            .context("corral command is missing")?
            .to_owned();
        let action = command
            .strip_prefix("corral.")
            .context("corral action is missing")?;
        let matching: Vec<_> = actual
            .iter()
            .filter(|entry| entry.get("key").and_then(Item::as_str) == Some(&key))
            .collect();
        let conflict =
            matching.iter().any(|entry| !same_binding(**entry, binding)) || native.contains(&key);
        if !conflict {
            continue;
        }
        // Reuse a previous safe repair while the preferred key still belongs to the user.
        let existing = actual.iter().find_map(|entry| {
            let candidate = entry.get("key")?.as_str()?;
            (entry.get("command").and_then(Item::as_str) == Some(&command)
                && entry.get("type").and_then(Item::as_str) == Some("plugin_action")
                && !reserved.contains(candidate)
                && !native.contains(candidate)
                && actual
                    .iter()
                    .filter(|other| other.get("key").and_then(Item::as_str) == Some(candidate))
                    .all(|other| same_binding(*other, *entry)))
            .then(|| candidate.to_owned())
        });
        let preferred = key
            .strip_prefix("prefix+")
            .map(|suffix| format!("prefix+{}", suffix.to_ascii_uppercase()));
        let mut candidates = preferred
            .into_iter()
            .chain((action == "pm").then(|| "prefix+p".to_owned()))
            .chain(('A'..='Z').map(|letter| format!("prefix+{letter}")));
        let alternative = existing
            .or_else(|| {
                candidates.find(|candidate| {
                    !occupied.contains(candidate) && !reserved.contains(candidate)
                })
            })
            .with_context(|| {
                format!(
                    "No free optional shortcut for {command}; set herdr.keys.{action} to a free key"
                )
            })?;
        warnings.push(format!(
            "{key} in {} is occupied; keep the user binding and use {alternative} for {command}. Set herdr.keys.{action} = {alternative:?} in .corral/corral.toml to prefer it; `corral doctor --fix` installs the alternative with a backup and diff.",
            path.display()
        ));
        occupied.insert(alternative.clone());
        reserved.insert(alternative.clone());
        binding["key"] = toml_edit::value(alternative);
    }
    Ok((wanted, warnings))
}

pub(super) fn merged_text(text: &str, path: &Path, keys: &HerdrKeys) -> Result<Option<String>> {
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;
    let (wanted, _) = resolved_bindings(&doc, path, keys)?;
    let expected = wanted["keys"]["command"]
        .as_array_of_tables()
        .context("corral keybindings must be tables")?;
    let actual = entries(&doc).with_context(|| format!("reading keys in {}", path.display()))?;
    let mut missing = Vec::new();
    for binding in expected {
        let key = binding.get("key").and_then(Item::as_str);
        let matching: Vec<_> = actual
            .iter()
            .filter(|entry| entry.get("key").and_then(Item::as_str) == key)
            .collect();
        if matching.is_empty() {
            missing.push(binding.clone());
        }
    }
    if missing.is_empty() {
        return Ok(None);
    }
    // Append to the original text for ordinary array-of-table configs: comments,
    // whitespace, dotted-key order and the entire unrelated prefix stay intact.
    if doc
        .get("keys")
        .and_then(|keys| keys.get("command"))
        .is_none_or(|item| item.is_array_of_tables())
        && doc.get("keys").is_none_or(Item::is_table)
    {
        let mut merged = text.to_owned();
        if !merged.is_empty() && !merged.ends_with('\n') {
            merged.push('\n');
        }
        for binding in missing {
            merged.push_str("\n[[keys.command]]\n");
            merged.push_str(&binding.to_string());
        }
        // Catch any structural incompatibility before making a backup or writing.
        merged
            .parse::<DocumentMut>()
            .context("checking merged herdr config")?;
        return Ok(Some(merged));
    }
    // Inline keys/command arrays cannot be extended by another table header.
    let keys = doc["keys"]
        .as_table_like_mut()
        .context("keys must be a table")?;
    if keys.get("command").is_none() {
        keys.insert("command", toml_edit::value(toml_edit::Array::new()));
    }
    let command = keys
        .get_mut("command")
        .and_then(Item::as_array_mut)
        .context("keys.command must be an array")?;
    for binding in missing {
        command.push(binding.into_inline_table());
    }
    Ok(Some(doc.to_string()))
}

/// Merges config, returning backup and diff messages without writing stdout.
pub fn merge_keybindings(keys: &HerdrKeys) -> Result<Vec<String>> {
    merge_at(&config_path()?, keys)
}

pub(super) fn merge_at(path: &Path, keys: &HerdrKeys) -> Result<Vec<String>> {
    let original = read_config(path)?;
    let before = original.as_deref().unwrap_or_default();
    let Some(after) = merged_text(before, path, keys)? else {
        return Ok(vec![format!(
            "keybindings already present in {}",
            path.display()
        )]);
    };
    let (_, warnings) = resolved_bindings(&before.parse::<DocumentMut>()?, path, keys)?;
    let mut messages: Vec<_> = warnings
        .into_iter()
        .map(|warning| format!("WARN keybindings: {warning}"))
        .collect();
    if original.is_some() {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{}.bak", jiff::Timestamp::now().as_nanosecond()));
        let backup = PathBuf::from(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .with_context(|| format!("creating backup {}", backup.display()))?;
        file.set_permissions(
            std::fs::metadata(path)
                .with_context(|| format!("reading metadata for {}", path.display()))?
                .permissions(),
        )
        .with_context(|| format!("setting permissions on {}", backup.display()))?;
        file.write_all(before.as_bytes())
            .with_context(|| format!("writing backup {}", backup.display()))?;
        messages.push(format!("backup: {}", backup.display()));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, &after).with_context(|| format!("writing {}", path.display()))?;
    let before_lines = before.lines().count();
    let after_lines = after.lines().count();
    let mut diff = format!(
        "--- {} (before)\n+++ {} (after)\n@@ -{},{} +{},{} @@\n",
        path.display(),
        path.display(),
        usize::from(before_lines != 0),
        before_lines,
        usize::from(after_lines != 0),
        after_lines
    );
    for (prefix, text) in [('-', before), ('+', after.as_str())] {
        for line in text.lines() {
            diff.push(prefix);
            diff.push_str(line);
            diff.push('\n');
        }
        if !text.is_empty() && !text.ends_with('\n') {
            diff.push_str("\\ No newline at end of file\n");
        }
    }
    messages.push(diff);
    Ok(messages)
}

#[cfg(test)]
#[path = "plugin_config_tests.rs"]
mod tests;

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, TableLike};

/// Resolves herdr's explicit override, XDG directory, then HOME/.config.
pub fn config_path() -> Result<PathBuf> {
    resolve_path(|name| std::env::var_os(name))
}

fn resolve_path(env: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    let nonempty = |name| env(name).filter(|value| !value.is_empty());
    if let Some(path) = nonempty("HERDR_CONFIG_PATH") {
        return Ok(path.into());
    }
    if let Some(path) = nonempty("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("herdr/config.toml"));
    }
    let home = nonempty("HOME").context("locating herdr config: HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/herdr/config.toml"))
}

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
pub fn keybindings_present() -> Result<bool> {
    present_at(&config_path()?)
}

fn present_at(path: &Path) -> Result<bool> {
    let text = read_config(path)?.unwrap_or_default();
    let doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;
    let wanted = super::keybindings().parse::<DocumentMut>()?;
    let actual = entries(&doc).with_context(|| format!("reading keys in {}", path.display()))?;
    Ok(entries(&wanted)?.iter().all(|expected| {
        let key = expected.get("key").and_then(Item::as_str);
        let matching: Vec<_> = actual
            .iter()
            .filter(|entry| entry.get("key").and_then(Item::as_str) == key)
            .collect();
        !matching.is_empty()
            && matching
                .iter()
                .all(|entry| same_binding(**entry, *expected))
    }))
}

pub(super) fn merged_text(text: &str, path: &Path) -> Result<Option<String>> {
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;
    let wanted = super::keybindings().parse::<DocumentMut>()?;
    let expected = wanted["keys"]["command"]
        .as_array_of_tables()
        .context("corral keybindings must be tables")?;
    let actual = entries(&doc).with_context(|| format!("reading keys in {}", path.display()))?;
    let mut missing = Vec::new();
    let mut conflicts = Vec::new();
    for binding in expected {
        let key = binding.get("key").and_then(Item::as_str);
        let matching: Vec<_> = actual
            .iter()
            .filter(|entry| entry.get("key").and_then(Item::as_str) == key)
            .collect();
        if matching.is_empty() {
            missing.push(binding.clone());
        } else if matching.iter().any(|entry| !same_binding(**entry, binding)) {
            conflicts.push(format!(
                "In {}, reassign or remove every keys.command entry with key = {}; then add:\n[[keys.command]]\n{binding}",
                path.display(), binding["key"]
            ));
        }
    }
    if !conflicts.is_empty() {
        bail!(
            "Conflicting user keybindings; config was not changed.\n{}",
            conflicts.join("\n")
        );
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
pub fn merge_keybindings() -> Result<Vec<String>> {
    merge_at(&config_path()?)
}

pub(super) fn merge_at(path: &Path) -> Result<Vec<String>> {
    let original = read_config(path)?;
    let before = original.as_deref().unwrap_or_default();
    let Some(after) = merged_text(before, path)? else {
        return Ok(vec![format!(
            "keybindings already present in {}",
            path.display()
        )]);
    };
    let mut messages = Vec::new();
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

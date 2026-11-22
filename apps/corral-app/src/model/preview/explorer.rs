//! Small, in-memory worktree for walking the explorer in explicit sample mode.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_protocol::detail::*;
use std::collections::BTreeSet;

const SAMPLE: &[(&str, &str, SessionFileKind)] = &[
    (
        "README.md",
        "# Sample workspace\n\nOpen reports/index.html for the standalone report.\n",
        SessionFileKind::Markdown,
    ),
    (
        "reports/index.html",
        "<!doctype html><title>Sample report</title><h1>Build report</h1><p>All checks passed.</p>",
        SessionFileKind::Html,
    ),
    (
        "src/main.rs",
        "fn main() {\n    println!(\"Sample workspace\");\n}\n",
        SessionFileKind::Text,
    ),
];

pub(super) fn listing(path: &str, filter: &str) -> Result<DetailBody, String> {
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    let mut rows = BTreeSet::new();
    let mut exists = false;
    let filter = filter.to_lowercase();
    for &(file, ..) in SAMPLE {
        if let Some(relative) = file.strip_prefix(&prefix) {
            exists = true;
            let (name, leaf) = relative
                .split_once('/')
                .map_or((relative, true), |(dir, _)| (dir, false));
            if name.to_lowercase().contains(&filter) {
                rows.insert((leaf, name));
            }
        }
    }
    if !exists {
        return Err("not_found: no such sample directory".into());
    }
    Ok(DetailBody::SessionFiles(SessionFiles {
        entries: rows
            .into_iter()
            .map(|(leaf, name)| SessionFileEntry {
                name: name.into(),
                directory: !leaf,
            })
            .collect(),
        truncated: false,
    }))
}

pub(super) fn file(path: &str) -> Result<DetailBody, String> {
    let &(_, source, kind) = SAMPLE
        .iter()
        .find(|(name, ..)| *name == path)
        .ok_or("not_found: no such sample file")?;
    Ok(DetailBody::SessionFile(SessionFile {
        kind,
        data: STANDARD.encode(source),
        bytes: source.len() as u64,
    }))
}

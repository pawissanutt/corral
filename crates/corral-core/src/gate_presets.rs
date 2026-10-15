//! Read-only suggestions inferred from repository files. Nothing is executed or applied.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// A suggested gate, ordered from repository-owned entry points to stack defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatePreset {
    pub name: String,
    pub commands: Vec<String>,
    /// Read-only repository content for the owner to inspect before choosing a gate.
    pub preview: Option<String>,
}

const READ_LIMIT: u64 = 128 * 1024;
const PREVIEW_LIMIT: usize = 16 * 1024;

fn read_optional(path: &Path) -> Result<Option<String>> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    // Do not follow symlinks or read devices/FIFOs while inspecting an unfamiliar checkout.
    if !metadata.is_file() {
        return Ok(None);
    }
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if u64::try_from(bytes.len())? > READ_LIMIT {
        bail!(
            "{} exceeds the 128 KiB preset inspection limit",
            path.display()
        );
    }
    String::from_utf8(bytes)
        .with_context(|| format!("{} is not UTF-8", path.display()))
        .map(Some)
}

fn preview(text: &str) -> String {
    if text.len() <= PREVIEW_LIMIT {
        return text.to_string();
    }
    let mut end = PREVIEW_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[preview truncated]", &text[..end])
}

fn preset(name: &str, commands: &[&str], text: Option<&str>) -> GatePreset {
    GatePreset {
        name: name.into(),
        commands: commands.iter().map(|s| (*s).into()).collect(),
        preview: text.map(preview),
    }
}

/// Detect suggested gate commands without running tools or modifying configuration.
///
/// Repository scripts come first, then Just recipes, Rust, JavaScript, Python and Go.
/// A nextest configuration adds an explicit alternative to the standard Cargo test gate.
///
/// # Errors
/// Returns an error for unreadable, oversized or malformed inspected configuration files.
pub fn detect_gate_presets(repo: &Path) -> Result<Vec<GatePreset>> {
    let mut presets = Vec::new();
    if let Some(script) = read_optional(&repo.join("scripts/gate.sh"))? {
        presets.push(preset(
            "Repository gate",
            &["bash scripts/gate.sh"],
            Some(&script),
        ));
    }
    for filename in ["justfile", "Justfile", ".justfile"] {
        if let Some(text) = read_optional(&repo.join(filename))? {
            for recipe in ["gate", "check"] {
                if text.lines().any(|line| {
                    !line.starts_with(char::is_whitespace)
                        && line.split_once(':').is_some_and(|(head, tail)| {
                            !tail.starts_with('=') && head.trim() == recipe
                        })
                }) {
                    presets.push(preset(
                        &format!("Just {recipe}"),
                        &[&format!("just {recipe}")],
                        Some(&text),
                    ));
                }
            }
            break;
        }
    }
    if read_optional(&repo.join("Cargo.toml"))?.is_some() {
        let commands = [
            "cargo fmt --all -- --check",
            "cargo clippy --all-targets -- -D warnings",
        ];
        presets.push(preset(
            "Rust",
            &[commands[0], commands[1], "cargo test"],
            None,
        ));
        if read_optional(&repo.join(".config/nextest.toml"))?.is_some() {
            presets.push(preset(
                "Rust (nextest)",
                &[commands[0], commands[1], "cargo nextest run"],
                None,
            ));
        }
    }
    if let Some(text) = read_optional(&repo.join("package.json"))? {
        let package: serde_json::Value =
            serde_json::from_str(&text).context("parsing package.json for gate presets")?;
        if let Some(scripts) = package.get("scripts").and_then(|v| v.as_object()) {
            let mut manager = "npm";
            for (lock, candidate) in [
                ("pnpm-lock.yaml", "pnpm"),
                ("package-lock.json", "npm"),
                ("npm-shrinkwrap.json", "npm"),
                ("yarn.lock", "yarn"),
                ("bun.lock", "bun"),
                ("bun.lockb", "bun"),
            ] {
                if repo.join(lock).is_file() {
                    manager = candidate;
                    break;
                }
            }
            let commands: Vec<String> = [
                "format:check",
                "typecheck",
                "lint",
                "check",
                "test",
                "build",
            ]
            .into_iter()
            .filter(|key| {
                scripts
                    .get(*key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
            })
            .map(|key| format!("{manager} run {key}"))
            .collect();
            if !commands.is_empty() {
                presets.push(GatePreset {
                    name: format!("JavaScript ({manager})"),
                    commands,
                    preview: None,
                });
            }
        }
    }
    let python = read_optional(&repo.join("pyproject.toml"))?;
    if let Some(text) = &python {
        let _: toml::Value =
            toml::from_str(text).context("parsing pyproject.toml for gate presets")?;
    }
    if python.is_some() || repo.join("pytest.ini").is_file() || repo.join("setup.cfg").is_file() {
        let uv = repo.join("uv.lock").is_file()
            || python
                .as_ref()
                .is_some_and(|text| text.contains("[tool.uv"));
        let prefix = if uv { "uv run " } else { "" };
        let mut commands = Vec::new();
        if repo.join("ruff.toml").is_file()
            || repo.join(".ruff.toml").is_file()
            || python.as_ref().is_some_and(|text| text.contains("ruff"))
        {
            commands.push(format!("{prefix}ruff format --check ."));
            commands.push(format!("{prefix}ruff check ."));
        }
        commands.push(format!("{prefix}pytest"));
        presets.push(GatePreset {
            name: "Python".into(),
            commands,
            preview: None,
        });
    }
    if read_optional(&repo.join("go.mod"))?.is_some() {
        presets.push(preset("Go", &["go vet ./...", "go test ./..."], None));
    }
    Ok(presets)
}

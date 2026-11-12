//! Declarative ownership of private, rebuildable outputs. No directory-name inference.
use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    CargoTarget,
    Generated,
}

/// An opt-in promise: only generated, disposable files are written here, by this task's
/// consumers. Corral registers an empty directory before submission, never adopts contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputCfg {
    pub kind: OutputKind,
    pub path: String,
    pub rebuildable: bool,
    pub exclusive: bool,
    #[serde(default = "grace")]
    pub grace_hours: u32,
    /// Generated output types (e.g. js, css, map, html). Not executable plugins or globs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
}

fn grace() -> u32 {
    24
}

/// These are not build outputs even inside an otherwise declared directory.
pub fn protected_component(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        ".git"
            | ".corral"
            | ".evidence"
            | "evidence"
            | ".omo"
            | "docs"
            | "src"
            | "reports"
            | "receipts"
            | "transcripts"
            | "credentials"
            | "secrets"
            | "node_modules"
            | ".cache"
            | ".venv"
            | "venv"
            | "site-packages"
            | "__pycache__"
            | ".worktrees"
            | ".netrc"
            | ".pypirc"
            | "id_rsa"
            | "id_ed25519"
            | "vendor"
            | ".cargo"
            | ".npm"
            | ".pnpm-store"
            | "auth.json"
            | "token.json"
            | "tokens.json"
            | "secrets.json"
            | "readme"
            | ".yarn"
            | "cache"
            | "caches"
            | "mbx"
            | ".ssh"
            | ".aws"
    ) || name.starts_with(".env")
        || name.contains("report")
        || name.contains("credential")
        || name.contains("transcript")
        || name.contains("receipt")
}

pub fn protected_extension(extension: &str) -> bool {
    matches!(
        extension,
        "rs" | "ts"
            | "tsx"
            | "jsx"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "java"
            | "kt"
            | "swift"
            | "svelte"
            | "vue"
            | "md"
            | "txt"
            | "log"
            | "jsonl"
            | "yaml"
            | "yml"
            | "toml"
            | "pem"
            | "key"
            | "sqlite"
            | "db"
    )
}

impl OutputCfg {
    /// Retention may change without adopting a different producer/path/content contract.
    pub fn ownership_eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.path == other.path
            && self.rebuildable == other.rebuildable
            && self.exclusive == other.exclusive
            && self.extensions == other.extensions
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.rebuildable && self.exclusive,
            "output must be rebuildable and exclusive"
        );
        ensure!(
            (1..=8760).contains(&self.grace_hours),
            "output grace_hours must be 1..8760"
        );
        let components = crate::fs_guard::plain_components(Path::new(&self.path))?;
        ensure!(components.len() <= 8, "output path is too deep");
        ensure!(self.path.len() <= 256, "output path is too long");
        for component in components {
            let name = component
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF-8 output path"))?;
            ensure!(
                !protected_component(name),
                "protected output component: {name}"
            );
        }
        ensure!(self.extensions.len() <= 16, "at most 16 output extensions");
        for extension in &self.extensions {
            ensure!(
                !extension.is_empty()
                    && extension.len() <= 16
                    && extension
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                    && !protected_extension(extension),
                "invalid or protected generated extension: {extension}"
            );
        }
        match self.kind {
            OutputKind::CargoTarget => {
                ensure!(
                    self.path == "target",
                    "Cargo target must be the private worktree target"
                );
                ensure!(
                    self.extensions.is_empty(),
                    "Cargo output has its own file contract"
                );
            }
            OutputKind::Generated => ensure!(
                !self.extensions.is_empty(),
                "generated output needs extensions"
            ),
        }
        Ok(())
    }
}

/// Cargo configuration is parsed, not substring-matched: quoted/escaped keys and env
/// configuration must not hide an output override. Unknown includes are refused too.
pub fn require_default_cargo_output(text: &str) -> Result<()> {
    fn check(value: &toml::Value) -> Result<()> {
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    let key = key.to_ascii_lowercase().replace('_', "-");
                    ensure!(
                        !key.contains("target-dir")
                            && !key.contains("build-dir")
                            && key != "include"
                            && key != "cargo-home",
                        "Cargo output/configuration override: {key}"
                    );
                    check(value)?;
                }
            }
            toml::Value::Array(values) => {
                for value in values {
                    check(value)?;
                }
            }
            toml::Value::String(_)
            | toml::Value::Integer(_)
            | toml::Value::Float(_)
            | toml::Value::Boolean(_)
            | toml::Value::Datetime(_) => {}
        }
        Ok(())
    }
    check(&toml::from_str::<toml::Value>(text)?)
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;

pub type ProjectOutputs = BTreeMap<String, BTreeMap<String, OutputCfg>>;

pub(crate) fn problems(
    outputs: &ProjectOutputs,
    projects: &BTreeMap<String, crate::Project>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (project, classes) in outputs {
        if !projects.contains_key(project) {
            problems.push(format!("storage.outputs.{project}: unknown project"));
        }
        if classes.len() > 8 {
            problems.push(format!(
                "storage.outputs.{project}: at most 8 output classes"
            ));
        }
        for (class, output) in classes {
            if class.is_empty()
                || class.len() > 32
                || !class
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                problems.push(format!("storage.outputs.{project}: invalid class {class}"));
            }
            if let Err(error) = output.validate() {
                problems.push(format!("storage.outputs.{project}.{class}: {error:#}"));
            }
            for (other, config) in classes.range(..class.clone()) {
                let (a, b) = (Path::new(&output.path), Path::new(&config.path));
                if a.starts_with(b) || b.starts_with(a) {
                    problems.push(format!(
                        "storage.outputs.{project}: overlapping {class} and {other}"
                    ));
                }
            }
        }
    }
    problems
}

//! Validate the candidate with the engine before atomically replacing corral.toml.
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use corral_engine::{Config, Refusal};
use corral_protocol::config::ConfigDocument;
use sha2::{Digest, Sha256};

use crate::state::Shared;

pub(crate) mod pi_startup;

fn path(root: &Path) -> PathBuf {
    root.join(crate::STATE_DIR).join("corral.toml")
}

pub(crate) fn fingerprint(root: &Path) -> Result<Option<[u8; 32]>> {
    let path = path(root);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(Sha256::digest(bytes).into())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn revision(toml: &str) -> String {
    format!("{:x}", Sha256::digest(toml.as_bytes()))
}

pub(crate) fn get(shared: &Shared) -> Result<ConfigDocument> {
    get_at(shared.root())
}

pub(crate) fn get_at(root: &Path) -> Result<ConfigDocument> {
    let path = path(root);
    let toml = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(ConfigDocument {
        revision: Some(revision(&toml)),
        toml,
    })
}

pub(crate) fn put(shared: &Shared, mut document: ConfigDocument) -> Result<ConfigDocument> {
    Config::check(shared.root(), &document.toml).map_err(|problems| {
        let mut reason = String::from("configuration problems:");
        for problem in &problems {
            reason.push_str(&format!("\n{problem}"));
        }
        Refusal {
            reason,
            next: Vec::new(),
            problems: Some(problems),
        }
    })?;
    let state = shared.root().join(crate::STATE_DIR);
    let path = path(shared.root());
    let mut file =
        tempfile::NamedTempFile::new_in(&state).context("create candidate configuration")?;
    file.write_all(document.toml.as_bytes())
        .context("write candidate configuration")?;
    let permissions = fs::metadata(&path)
        .context("read current configuration permissions")?
        .permissions();
    file.as_file()
        .set_permissions(permissions)
        .context("set candidate configuration permissions")?;
    shared.update_config_fingerprint(|| {
        file.persist(&path).context("replace corral.toml")?;
        Ok(Some(Sha256::digest(document.toml.as_bytes()).into()))
    })?;
    document.revision = Some(revision(&document.toml));
    Ok(document)
}

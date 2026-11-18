//! Validate the candidate with the engine before atomically replacing corral.toml.
use std::{fs, io::Write};

use anyhow::{Context, Result};
use corral_engine::{Config, refuse};
use corral_protocol::config::ConfigDocument;

use crate::state::Shared;

pub(crate) fn get(shared: &Shared) -> Result<ConfigDocument> {
    let path = shared.root().join(crate::STATE_DIR).join("corral.toml");
    let toml = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(ConfigDocument { toml })
}

pub(crate) fn put(shared: &Shared, document: ConfigDocument) -> Result<ConfigDocument> {
    let state = shared.root().join(crate::STATE_DIR);
    let path = state.join("corral.toml");
    let mut file =
        tempfile::NamedTempFile::new_in(&state).context("create candidate configuration")?;
    file.write_all(document.toml.as_bytes())
        .context("write candidate configuration")?;
    let candidate =
        Config::load(file.path()).map_err(|error| refuse(format!("{error:#}"), vec![]))?;
    let problems = candidate.validate(shared.root());
    if !problems.is_empty() {
        return Err(refuse(problems.join("\n"), vec![]));
    }
    let permissions = fs::metadata(&path)
        .context("read current configuration permissions")?
        .permissions();
    file.as_file()
        .set_permissions(permissions)
        .context("set candidate configuration permissions")?;
    file.persist(&path).context("replace corral.toml")?;
    shared.publish();
    shared.nudge(None);
    Ok(document)
}

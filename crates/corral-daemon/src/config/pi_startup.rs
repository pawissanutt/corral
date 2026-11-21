//! The app edits one Pi setting, never the credential-bearing document.
use std::{
    fs::{File, Permissions},
    io::{Read, Write},
    path::Path,
    sync::Mutex,
};

use anyhow::{Context, Result, bail, ensure};
use corral_protocol::config::{PiStartupConfig, PiStartupRequest};
use rustix::fs::{Mode, OFlags, open};
use toml_edit::{DocumentMut, Item, Table, Value};

const MAX_BYTES: u64 = 256 * 1024;
static WRITES: Mutex<()> = Mutex::new(());

fn read(path: &Path) -> Result<(String, Option<Permissions>)> {
    let file = match open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok((String::new(), None)),
        Err(error) => return Err(error).context("cannot open .corral-next/pi.toml"),
    };
    let metadata = file.metadata().context("cannot inspect pi.toml")?;
    ensure!(metadata.is_file(), "pi.toml is not a regular file");
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .context("cannot read pi.toml as UTF-8")?;
    ensure!(text.len() as u64 <= MAX_BYTES, "pi.toml is too large");
    Ok((text, Some(metadata.permissions())))
}

fn revision(text: &str, exists: bool) -> String {
    if exists {
        super::revision(text)
    } else {
        "missing".into()
    }
}

fn valid_model(model: &str) -> bool {
    model.len() <= 256
        && !model.chars().any(|c| c.is_whitespace() || c.is_control())
        && model.split_once('/').is_some_and(|(provider, id)| {
            // Keep in sync with the launch routes in corral-pi/src/auth/providers.ts.
            !id.is_empty()
                && matches!(
                    provider,
                    "openai-codex"
                        | "openai"
                        | "anthropic"
                        | "anthropic-api"
                        | "amazon-bedrock"
                        | "llama.cpp"
                )
        })
}

pub(crate) fn handle(root: &Path, request: PiStartupRequest) -> Result<PiStartupConfig> {
    let _guard = if request.change.is_some() {
        Some(
            WRITES
                .lock()
                .map_err(|_| anyhow::anyhow!("Pi settings writer unavailable"))?,
        )
    } else {
        None
    };
    let state = root.join(crate::STATE_DIR);
    let path = state.join("pi.toml");
    let (text, permissions) = read(&path)?;
    let before = revision(&text, permissions.is_some());
    // Parser errors can quote lines containing secrets; never return them to an app.
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|_| anyhow::anyhow!("pi.toml contains invalid TOML"))?;
    let startup = doc
        .get("startup")
        .map(|item| {
            item.as_table_like()
                .context("pi.toml startup must be a table")
        })
        .transpose()?;
    let Some(change) = request.change else {
        let model = startup
            .and_then(|table| table.get("model"))
            .map(|item| {
                item.as_str()
                    .filter(|model| valid_model(model))
                    .map(str::to_owned)
                    .context("pi.toml startup.model is invalid; correct it on the daemon host")
            })
            .transpose()?;
        return Ok(PiStartupConfig {
            revision: before,
            model,
        });
    };
    ensure!(
        change.revision == before,
        "pi.toml changed; reload the startup setting before saving"
    );
    if let Some(model) = &change.model {
        ensure!(
            valid_model(model),
            "Enter a known Pi provider followed by / and a model ID"
        );
        if doc.get("startup").is_none() {
            doc.insert("startup", Item::Table(Table::new()));
        }
        let table = doc
            .get_mut("startup")
            .and_then(Item::as_table_like_mut)
            .context("pi.toml startup must be a table")?;
        let mut value = Value::from(model.as_str());
        if let Some(old) = table.get("model").and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        table.insert("model", Item::Value(value));
    } else if let Some(table) = doc.get_mut("startup").and_then(Item::as_table_like_mut) {
        table.remove("model");
    }
    let updated = doc.to_string();
    if updated == text {
        return Ok(PiStartupConfig {
            revision: before,
            model: change.model,
        });
    }
    ensure!(updated.len() as u64 <= MAX_BYTES, "pi.toml is too large");
    let mut temp =
        tempfile::NamedTempFile::new_in(&state).context("create Pi configuration candidate")?;
    temp.write_all(updated.as_bytes())
        .context("write Pi configuration candidate")?;
    let exists = permissions.is_some();
    if let Some(permissions) = permissions {
        temp.as_file()
            .set_permissions(permissions)
            .context("preserve Pi configuration permissions")?;
    }
    let (current, current_permissions) = read(&path)?;
    if revision(&current, current_permissions.is_some()) != before {
        bail!("pi.toml changed; reload the startup setting before saving");
    }
    if exists {
        temp.persist(&path).context("replace pi.toml")?;
    } else {
        temp.persist_noclobber(&path)
            .context("create pi.toml; reload if it has changed")?;
    }
    Ok(PiStartupConfig {
        revision: revision(&updated, true),
        model: change.model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::config::PiStartupChange;
    use std::fs;

    #[test]
    fn startup_edits_only_the_model_and_refuses_stale_or_unsafe_writes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join(crate::STATE_DIR);
        fs::create_dir(&state)?;
        let path = state.join("pi.toml");
        let read = || handle(temp.path(), PiStartupRequest::default());
        let write = |revision: String, model: Option<&str>| {
            handle(
                temp.path(),
                PiStartupRequest {
                    change: Some(PiStartupChange {
                        revision,
                        model: model.map(str::to_owned),
                    }),
                },
            )
        };
        let missing = read()?;
        assert_eq!(missing.model, None);
        let created = write(missing.revision, Some("openai-codex/gpt-6.2-sol"))?;
        assert_eq!(created, read()?);
        fs::write(
            &path,
            "[mcp]\nother = 'keep'\n[startup]\nmodel = 'openai-codex/example' # note\nfuture = true\n",
        )?;
        assert!(write(created.revision, None).is_err());
        let current = read()?;
        assert!(write(current.revision.clone(), Some("unknown/model")).is_err());
        let saved = write(current.revision, Some("llama.cpp/local"))?;
        assert_eq!(saved, read()?);
        let text = fs::read_to_string(&path)?;
        assert!(text.contains("other = 'keep'") && text.contains("# note"));
        assert_eq!(write(saved.revision, None)?.model, None);
        assert!(fs::read_to_string(&path)?.contains("future = true"));
        fs::remove_file(&path)?;
        std::os::unix::fs::symlink("missing-target", &path)?;
        assert!(read().is_err());
        Ok(())
    }
}

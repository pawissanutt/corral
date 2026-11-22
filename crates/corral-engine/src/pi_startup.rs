//! The shared writer for one Pi preference, never the credential-bearing document.
use std::{
    fs::{File, Metadata, TryLockError},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use rustix::fs::{AtFlags, Mode, OFlags, linkat, open, openat, renameat, unlinkat};
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table, Value};

const FILE: &str = "pi.toml";
const MAX_BYTES: u64 = 256 * 1024;
const STALE: &str = "pi.toml changed; reload the startup setting before saving";

#[derive(Debug, PartialEq, Eq)]
pub struct Preference {
    pub revision: String,
    pub model: Option<String>,
}

fn open_state(root: &Path) -> Result<File> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let root = open(root, flags, Mode::empty()).context("open Pi workspace root")?;
    openat(root, ".corral-next", flags, Mode::empty())
        .map(File::from)
        .context("open workspace .corral-next directory")
}

fn read_file(state: &File) -> Result<(String, Option<Metadata>)> {
    let file = match openat(
        state,
        FILE,
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
    Ok((text, Some(metadata)))
}

fn revision(text: &str, exists: bool) -> String {
    if exists {
        format!("{:x}", Sha256::digest(text.as_bytes()))
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

fn parse(text: &str) -> Result<DocumentMut> {
    // Parser errors can quote lines containing secrets; never return them.
    let doc: DocumentMut = text
        .parse()
        .map_err(|_| anyhow::anyhow!("pi.toml contains invalid TOML"))?;
    ensure!(
        doc.get("startup").is_none_or(|item| item.is_table_like()),
        "pi.toml startup must be a table"
    );
    Ok(doc)
}

pub fn read(root: &Path) -> Result<Preference> {
    let state = open_state(root)?;
    let (text, metadata) = read_file(&state)?;
    let doc = parse(&text)?;
    let model = doc
        .get("startup")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("model"))
        .map(|item| {
            item.as_str()
                .filter(|model| valid_model(model))
                .map(str::to_owned)
                .context("pi.toml startup.model is invalid; correct it on the workspace host")
        })
        .transpose()?;
    Ok(Preference {
        revision: revision(&text, metadata.is_some()),
        model,
    })
}

pub fn save(root: &Path, expected: &str, model: Option<String>) -> Result<Preference> {
    let state = open_state(root)?;
    // Lock the stable directory inode, not pi.toml (which we replace). The lock spans
    // validation and publication in both CLI and daemon, and needs no sidecar file.
    match state.try_lock() {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => bail!("Pi settings are being saved; reload and try again"),
        Err(error) => return Err(error).context("lock Pi settings for writing"),
    }
    let (text, metadata) = read_file(&state)?;
    let before = revision(&text, metadata.is_some());
    ensure!(expected == before, "{STALE}");
    let mut doc = parse(&text)?;
    if let Some(model) = &model {
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
        return Ok(Preference {
            revision: before,
            model,
        });
    }
    ensure!(updated.len() as u64 <= MAX_BYTES, "pi.toml is too large");
    let token = getrandom::u64().context("name Pi configuration candidate")?;
    let name = format!(".pi-startup-{token:016x}.tmp");
    let mut file = File::from(
        openat(
            &state,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .context("create Pi configuration candidate")?,
    );
    let temp = Candidate {
        state: &state,
        name,
    };
    file.write_all(updated.as_bytes())
        .context("write Pi configuration candidate")?;
    if let Some(original) = &metadata {
        file.set_permissions(original.permissions())
            .context("preserve Pi configuration permissions")?;
    }
    file.sync_all().context("sync Pi configuration candidate")?;
    // External editors need not take our lock. Re-check immediately before replacing,
    // including identity and mode so a same-content replacement is not silently discarded.
    let (current, current_metadata) = read_file(&state)?;
    let fingerprint = |meta: &Metadata| (meta.dev(), meta.ino(), meta.mode());
    ensure!(
        current == text
            && current_metadata.as_ref().map(fingerprint) == metadata.as_ref().map(fingerprint),
        "{STALE}"
    );
    if metadata.is_some() {
        renameat(&state, temp.name.as_str(), &state, FILE).context("replace pi.toml")?;
    } else {
        linkat(&state, temp.name.as_str(), &state, FILE, AtFlags::empty())
            .context("create pi.toml; reload if it has changed")?;
    }
    Ok(Preference {
        revision: revision(&updated, true),
        model,
    })
}

// All candidate IO (including cleanup) is relative to the open state directory, so
// a directory being swapped for a symlink cannot redirect the atomic replacement.
struct Candidate<'a> {
    state: &'a File,
    name: String,
}

impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        let _ = unlinkat(self.state, self.name.as_str(), AtFlags::empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn startup_edits_only_the_model_and_refuses_stale_or_unsafe_writes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join(".corral-next");
        fs::create_dir(&state)?;
        let path = state.join(FILE);
        let read = || read(temp.path());
        let write = |revision: String, model: Option<&str>| {
            save(temp.path(), &revision, model.map(str::to_owned))
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
        symlink("missing-target", &path)?;
        assert!(read().is_err());
        fs::remove_file(&path)?;
        let moved = temp.path().join("elsewhere");
        fs::rename(&state, &moved)?;
        symlink(&moved, &state)?;
        assert!(read().is_err());
        assert!(write("missing".into(), Some("openai/model")).is_err());
        assert!(!moved.join(FILE).exists());
        Ok(())
    }
}

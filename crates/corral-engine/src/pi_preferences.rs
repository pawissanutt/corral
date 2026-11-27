//! Descriptor-relative and revision-checked access to the private Pi document.
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

pub(crate) const FILE: &str = "pi.toml";
const MAX_BYTES: u64 = 256 * 1024;

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

fn parse(text: &str) -> Result<DocumentMut> {
    // TOML parser diagnostics may quote lines containing secrets.
    text.parse()
        .map_err(|_| anyhow::anyhow!("pi.toml contains invalid TOML"))
}

pub fn read(root: &Path) -> Result<(String, DocumentMut)> {
    let state = open_state(root)?;
    let (text, metadata) = read_file(&state)?;
    Ok((revision(&text, metadata.is_some()), parse(&text)?))
}

pub(crate) fn set_string(
    doc: &mut DocumentMut,
    section: &str,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    if let Some(value) = value {
        if doc.get(section).is_none() {
            doc.insert(section, Item::Table(Table::new()));
        }
        // Callers check the section shape before editing.
        let table = doc
            .get_mut(section)
            .and_then(Item::as_table_like_mut)
            .context("Pi preference section is not a table")?;
        let mut value = Value::from(value);
        if let Some(old) = table.get(key).and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        table.insert(key, Item::Value(value));
    } else if let Some(table) = doc.get_mut(section).and_then(Item::as_table_like_mut) {
        table.remove(key);
    }
    Ok(())
}

// Explicitly unlock even if a forked child temporarily retains this open file description.
struct SaveLock<'a>(&'a File);

impl Drop for SaveLock<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub fn save(
    root: &Path,
    expected: &str,
    stale_error: &str,
    edit: impl FnOnce(&mut DocumentMut) -> Result<()>,
) -> Result<String> {
    let state = open_state(root)?;
    // Lock the stable directory inode, not pi.toml (which we replace).
    match state.try_lock() {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => bail!("Pi settings are being saved; reload and try again"),
        Err(error) => return Err(error).context("lock Pi settings for writing"),
    }
    let _lock = SaveLock(&state);
    let (text, metadata) = read_file(&state)?;
    let before = revision(&text, metadata.is_some());
    ensure!(expected == before, "{stale_error}");
    let mut doc = parse(&text)?;
    edit(&mut doc)?;
    let updated = doc.to_string();
    if updated == text {
        return Ok(before);
    }
    ensure!(updated.len() as u64 <= MAX_BYTES, "pi.toml is too large");
    let token = getrandom::u64().context("name Pi configuration candidate")?;
    let name = format!(".pi-preferences-{token:016x}.tmp");
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
        "{stale_error}"
    );
    if metadata.is_some() {
        renameat(&state, temp.name.as_str(), &state, FILE).context("replace pi.toml")?;
    } else {
        linkat(&state, temp.name.as_str(), &state, FILE, AtFlags::empty())
            .context("create pi.toml; reload if it has changed")?;
    }
    Ok(revision(&updated, true))
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

    #[test]
    fn pi_startup_inherited_descriptor_does_not_retain_finished_save_lock() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir(temp.path().join(".corral-next"))?;
        let state = open_state(temp.path())?;
        state.try_lock()?;
        let inherited = state.try_clone()?;
        let lock = SaveLock(&state);
        drop(lock);
        drop(state);
        let other = open_state(temp.path())?;
        assert!(other.try_lock().is_ok(), "finished save retained its lock");
        drop(inherited);
        Ok(())
    }
}

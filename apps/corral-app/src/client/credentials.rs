//! Native device-id file fallback. It contains no daemon or browser private keys.
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Credentials {
    version: u32,
    devices: BTreeMap<String, String>,
}
impl Default for Credentials {
    fn default() -> Self {
        Self {
            version: 1,
            devices: BTreeMap::new(),
        }
    }
}

fn state_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        let root = PathBuf::from(root);
        if root.is_absolute() {
            return Ok(root);
        }
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".local/state"))
        .ok_or_else(|| "cannot store the device id: set XDG_STATE_HOME or HOME".into())
}

fn read(root: &Path) -> Result<Credentials, String> {
    let path = root.join("corral-app/devices.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Credentials::default());
        }
        Err(error) => return Err(format!("cannot inspect devices.json: {error}")),
    };
    if !metadata.is_file() {
        return Err("devices.json must be a regular file".into());
    }
    #[cfg(unix)]
    if metadata.mode() & 0o077 != 0 || metadata.uid() != rustix::process::getuid().as_raw() {
        return Err("devices.json must be readable only by you".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32);
    let file = options
        .open(path)
        .map_err(|error| format!("cannot read devices.json: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect devices.json: {error}"))?;
    if !metadata.is_file() {
        return Err("devices.json must be a regular file".into());
    }
    #[cfg(unix)]
    if metadata.mode() & 0o077 != 0 || metadata.uid() != rustix::process::getuid().as_raw() {
        return Err("devices.json must be readable only by you".into());
    }
    let saved: Credentials = serde_json::from_reader(file)
        .map_err(|error| format!("cannot decode devices.json: {error}"))?;
    if saved.version != 1 {
        return Err("devices.json has an unsupported version".into());
    }
    Ok(saved)
}

fn write(root: &Path, saved: &Credentials) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("cannot create device state directory: {error}"))?;
    let dir = root.join("corral-app");
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot create corral-app state directory: {error}")),
    }
    let metadata = fs::symlink_metadata(&dir).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err("corral-app must be a private directory owned by you".into());
    }
    #[cfg(unix)]
    if metadata.mode() & 0o077 != 0 || metadata.uid() != rustix::process::getuid().as_raw() {
        return Err("corral-app must be a private directory owned by you".into());
    }
    #[cfg(not(unix))]
    return Err("private device-id file storage is unavailable on this platform".into());
    #[cfg(unix)]
    {
        let stage = dir.join(format!("devices.json.{}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&stage)
            .map_err(|error| format!("cannot create private device-id file: {error}"))?;
        let result = (|| {
            let bytes = serde_json::to_vec(saved).map_err(|error| error.to_string())?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            fs::rename(&stage, dir.join("devices.json")).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(stage);
        }
        result.map_err(|error| format!("cannot save devices.json: {error}"))
    }
}

pub(super) fn load_in(root: &Path, store: &str) -> Result<Option<String>, String> {
    Ok(read(root)?.devices.remove(store))
}
pub(super) fn save_in(root: &Path, store: &str, device: &str) -> Result<(), String> {
    let mut saved = read(root)?;
    saved.devices.insert(store.into(), device.into());
    write(root, &saved)
}
pub(super) fn remove_in(root: &Path, store: &str) -> Result<(), String> {
    let mut saved = read(root)?;
    saved.devices.remove(store);
    write(root, &saved)
}
pub(super) fn load(store: &str) -> Result<Option<String>, String> {
    load_in(&state_root()?, store)
}
pub(super) fn save(store: &str, device: &str) -> Result<(), String> {
    save_in(&state_root()?, store, device)
}
pub(super) fn remove(store: &str) -> Result<(), String> {
    remove_in(&state_root()?, store)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn saved_device_file_is_private_and_reloads() {
        let root = std::env::temp_dir().join(format!(
            "corral-app-credentials-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        save_in(&root, "workspace-a", "dev-first").unwrap();
        let path = root.join("corral-app/devices.json");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            load_in(&root, "workspace-a").unwrap().as_deref(),
            Some("dev-first")
        );
        save_in(&root, "workspace-a", "dev-replaced").unwrap();
        save_in(&root, "workspace-b", "dev-second").unwrap();
        assert_eq!(
            load_in(&root, "workspace-a").unwrap().as_deref(),
            Some("dev-replaced")
        );
        remove_in(&root, "workspace-a").unwrap();
        assert_eq!(load_in(&root, "workspace-a").unwrap(), None);
        assert_eq!(
            load_in(&root, "workspace-b").unwrap().as_deref(),
            Some("dev-second")
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            load_in(&root, "workspace-b").unwrap_err(),
            "devices.json must be readable only by you"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

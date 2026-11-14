//! Persistent per-workspace Noise identity, kept outside the workspace database.
use std::env;
use std::fs::{self, Metadata, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use corral_protocol::device::NOISE_SESSION;

#[derive(Clone, Copy)]
pub(crate) struct Identity {
    pub private: [u8; 32],
    pub public: [u8; 32],
}

pub(crate) fn state_root() -> Result<PathBuf> {
    if let Some(root) = env::var_os("XDG_STATE_HOME").filter(|root| !root.is_empty()) {
        let root = PathBuf::from(root);
        if root.is_absolute() {
            return Ok(root);
        }
    }
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".local/state"))
        .context("app identity needs XDG_STATE_HOME or HOME")
}

pub(crate) fn load_or_create(state_root: &Path, store_id: &str) -> Result<Identity> {
    fs::create_dir_all(state_root).with_context(|| format!("creating {}", state_root.display()))?;
    let root = state_root.join("corral");
    let dir = root.join(store_id);
    corral_ops::agent::launch::ensure_private_dir(&root, &dir)?;
    let path = dir.join("identity.key");
    if let Some(identity) = read_existing(&path)? {
        return Ok(identity);
    }

    let keys = snow::Builder::new(NOISE_SESSION.parse()?)
        .generate_keypair()
        .context("generating daemon Noise identity")?;
    let identity = Identity {
        private: keys
            .private
            .as_slice()
            .try_into()
            .context("Noise generated an invalid private key length")?,
        public: keys
            .public
            .as_slice()
            .try_into()
            .context("Noise generated an invalid public key length")?,
    };
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return read_existing(&path)?.with_context(|| {
                format!(
                    "{} disappeared while opening the daemon identity",
                    path.display()
                )
            });
        }
        Err(error) => {
            return Err(error).with_context(|| format!("creating {}", path.display()));
        }
    };
    file.write_all(&identity.private)
        .and_then(|()| file.write_all(&identity.public))
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    Ok(identity)
}

fn validate(meta: &Metadata, path: &Path) -> Result<()> {
    ensure!(
        meta.is_file()
            && meta.uid() == rustix::process::getuid().as_raw()
            && meta.mode() & 0o077 == 0
            && meta.len() == 64,
        "{} must be a 64-byte file readable only by you",
        path.display()
    );
    Ok(())
}

fn read_existing(path: &Path) -> Result<Option<Identity>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", path.display()));
        }
    };
    validate(&meta, path)?;
    // NOFOLLOW refuses a raced symlink; NONBLOCK keeps a raced FIFO from blocking.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    validate(
        &file
            .metadata()
            .with_context(|| format!("inspecting {}", path.display()))?,
        path,
    )?;
    let mut bytes = [0; 64];
    if let Err(error) = file.read_exact(&mut bytes) {
        if error.kind() == ErrorKind::UnexpectedEof {
            bail!(
                "{} must be a 64-byte file readable only by you",
                path.display()
            );
        }
        return Err(error).with_context(|| format!("reading {}", path.display()));
    }
    // Detect growth or permission changes without reading past the 64-byte bound.
    validate(
        &file
            .metadata()
            .with_context(|| format!("inspecting {}", path.display()))?,
        path,
    )?;
    let mut identity = Identity {
        private: [0; 32],
        public: [0; 32],
    };
    identity.private.copy_from_slice(&bytes[..32]);
    identity.public.copy_from_slice(&bytes[32..]);
    Ok(Some(identity))
}

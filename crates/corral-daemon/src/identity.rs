//! Persistent per-workspace Noise identity, kept outside the workspace database.
use std::fs::{File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use corral_protocol::device::NOISE_SESSION;

#[derive(Clone, Copy)]
pub(crate) struct Identity {
    pub private: [u8; 32],
    pub public: [u8; 32],
}

pub(crate) fn load_or_create(state_root: &Path) -> Result<Identity> {
    let path = state_root.join("identity.key");
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
    // Only expose a complete identity. A competing opener uses the winner's key.
    let mut temp = tempfile::NamedTempFile::new_in(state_root)
        .context("creating a temporary daemon Noise identity")?;
    temp.write_all(&identity.private)
        .and_then(|()| temp.write_all(&identity.public))
        .with_context(|| format!("writing {}", path.display()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    match temp.persist_noclobber(&path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
            return read_existing(&path)?.with_context(|| {
                format!(
                    "{} disappeared while opening the daemon identity",
                    path.display()
                )
            });
        }
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    }
    Ok(identity)
}

fn validate(meta: &Metadata, path: &Path) -> Result<()> {
    ensure!(
        is_private(meta) && meta.len() == 64,
        "{} must be a 64-byte file readable only by you",
        path.display()
    );
    Ok(())
}

fn is_private(meta: &Metadata) -> bool {
    meta.is_file() && meta.uid() == rustix::process::getuid().as_raw() && meta.mode() & 0o077 == 0
}

pub(super) fn open_private(path: &Path) -> Result<Option<File>> {
    // NOFOLLOW refuses symlinks; NONBLOCK keeps a FIFO from blocking.
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let meta = file
        .metadata()
        .with_context(|| format!("inspecting {}", path.display()))?;
    ensure!(
        is_private(&meta),
        "{} must be a regular file readable only by you",
        path.display()
    );
    Ok(Some(file))
}

fn read_existing(path: &Path) -> Result<Option<Identity>> {
    let Some(mut file) = open_private(path)? else {
        return Ok(None);
    };
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

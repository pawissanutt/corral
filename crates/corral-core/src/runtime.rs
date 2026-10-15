//! Fail closed when a long-running process outlives its installed executable.
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};

pub const UPDATE_NOTICE: &str = "corral was updated; restart the board (q, then corral up)";

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    modified: SystemTime,
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Fingerprint {
    fn read(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("inspecting executable {}", path.display()))?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            modified: meta
                .modified()
                .context("reading executable modification time")?,
            len: meta.len(),
            #[cfg(unix)]
            device: meta.dev(),
            #[cfg(unix)]
            inode: meta.ino(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct Executable {
    path: PathBuf,
    running: Fingerprint,
}

impl Executable {
    fn capture() -> Result<Self> {
        let path = std::env::current_exe().context("locating running executable")?;
        // /proc/self/exe still refers to the running inode even after unlink/replacement.
        #[cfg(target_os = "linux")]
        let running = Fingerprint::read(Path::new("/proc/self/exe"))?;
        #[cfg(not(target_os = "linux"))]
        let running = Fingerprint::read(&path)?;
        Ok(Self { path, running })
    }

    pub(crate) fn current() -> Result<Arc<Self>> {
        static RUNNING: OnceLock<Arc<Executable>> = OnceLock::new();
        if let Some(exe) = RUNNING.get() {
            return Ok(Arc::clone(exe));
        }
        let exe = Arc::new(Self::capture()?);
        Ok(Arc::clone(RUNNING.get_or_init(|| exe)))
    }

    pub(crate) fn check(&self) -> Result<()> {
        if Fingerprint::read(&self.path).ok().as_ref() != Some(&self.running) {
            bail!("{UPDATE_NOTICE}");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fixture(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            running: Fingerprint::read(path)?,
        })
    }
}

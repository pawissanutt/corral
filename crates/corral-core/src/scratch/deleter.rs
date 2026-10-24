use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::fs_guard::{DirHandle, Removal, plain_components};
use crate::storage_roots::{StorageClass, StorageLayout};

/// One registered entry: an id, the class that owns it, and its path relative to that class's
/// root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub class: StorageClass,
    pub id: String,
    pub rel: PathBuf,
}

/// The one deleter of a storage class (plan section 8): it holds a no-follow handle on its class
/// root and deletes only registered entries of its own class through it.
#[derive(Debug)]
pub struct ClassDeleter {
    class: StorageClass,
    root: DirHandle,
}

impl ClassDeleter {
    /// The deleter of `class`, rooted at that class's validated root, which must already exist.
    pub fn open(layout: &StorageLayout, class: StorageClass) -> Result<Self> {
        Self::open_at(layout, class, &layout.class_root(class))
    }

    /// Opens a deleter for `class` at `root`, refusing any root other than the class's own:
    /// containers, other class roots, `C/lock` and `C/rollback` are never accepted.
    pub fn open_at(layout: &StorageLayout, class: StorageClass, root: &Path) -> Result<Self> {
        if let Some(name) = layout.protected(root) {
            bail!(
                "no {class} deleter accepts {} ({name}): containers and other class roots are never deletion roots",
                root.display()
            );
        }
        let own = layout.class_root(class);
        if root != own {
            bail!(
                "{} is not the {class} class root {}",
                root.display(),
                own.display()
            );
        }
        Ok(Self {
            class,
            root: DirHandle::open(root)?,
        })
    }

    pub fn class(&self) -> StorageClass {
        self.class
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Refusals that need no filesystem access: another class's entry, or a path that is
    /// empty, absolute or not made of plain names.
    pub fn preflight(&self, entry: &Registration) -> Result<()> {
        if entry.class != self.class {
            bail!(
                "entry {} belongs to the {} class; the {} deleter refuses it",
                entry.id,
                entry.class,
                self.class
            );
        }
        plain_components(&entry.rel)?;
        Ok(())
    }

    pub fn create(&self, entry: &Registration) -> Result<PathBuf> {
        self.preflight(entry)?;
        self.root.create_entry(&entry.rel)?;
        Ok(self.root.path().join(&entry.rel))
    }

    /// Deletes `entry` through the root handle. Immediately before deleting, the root's
    /// `(dev, inode)` is re-validated and `still_registered` re-checks the registration.
    pub fn remove(
        &self,
        entry: &Registration,
        still_registered: &dyn Fn() -> Result<()>,
    ) -> Result<Removal> {
        self.preflight(entry)?;
        self.root.remove_entry(&entry.rel, still_registered)
    }
}

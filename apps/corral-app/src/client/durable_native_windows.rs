//! Shared per-user file transactions for durable records and the Windows pairing store.
use std::{
    env, fs,
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

pub(in crate::client) fn state_root() -> Result<PathBuf, String> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or_else(|| {
            "Windows user state directory is unavailable: LOCALAPPDATA is not an absolute path"
                .into()
        })
}

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

pub(in crate::client) struct LockedFile {
    path: PathBuf,
    name: &'static str,
    _lock: fs::File,
    _directory: fs::File,
}

fn regular_file_size(file: &fs::File) -> io::Result<u64> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the file owns a live handle, and the output points to a writable Win32 struct.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes
        & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_DEVICE | FILE_ATTRIBUTE_REPARSE_POINT)
        != 0
        || info.nNumberOfLinks != 1
    {
        return Err(io::Error::other("must be a regular file without links"));
    }
    Ok((u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow))
}

impl LockedFile {
    pub(in crate::client) fn lock(
        root: &Path,
        name: &'static str,
        lock_name: &str,
    ) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let directory = root.canonicalize()?.join("corral-app");
        match fs::create_dir(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        // The user profile supplies the ACL. Do not follow a reparse point or allow the
        // directory or lock to be replaced while this transaction is using it.
        let dir = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&directory)?;
        let metadata = dir.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other(
                "corral-app must be a directory without links",
            ));
        }
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(directory.join(lock_name))?;
        regular_file_size(&lock)?;
        lock.lock()?;
        Ok(Self {
            path: directory.join(name),
            name,
            _lock: lock,
            _directory: dir,
        })
    }

    fn existing(&self) -> io::Result<Option<(fs::File, u64)>> {
        let file = match fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let size = regular_file_size(&file)?;
        Ok(Some((file, size)))
    }

    pub(in crate::client) fn read(
        &self,
        limit: usize,
        too_large: &'static str,
    ) -> io::Result<Option<Vec<u8>>> {
        let Some((file, size)) = self.existing()? else {
            return Ok(None);
        };
        if size > limit as u64 {
            return Err(io::Error::other(too_large));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(io::Error::other(too_large));
        }
        Ok(Some(bytes))
    }

    pub(in crate::client) fn replace(&self, bytes: &[u8]) -> io::Result<()> {
        // Forget can replace undecodable bytes, but may not override an unsafe file path.
        drop(self.existing()?);
        let (stage, mut file) = loop {
            let stage = self.path.with_file_name(format!(
                "{}.{}.{}.tmp",
                self.name,
                std::process::id(),
                NEXT_STAGE.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .share_mode(0)
                .open(&stage)
            {
                Ok(file) => break (stage, file),
                // A crashed process can leave a stage with a reused PID.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let result = result.and_then(|()| replace_file(&stage, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(stage);
        }
        result
    }
}

fn replace_file(stage: &Path, destination: &Path) -> io::Result<()> {
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let stage = wide(stage);
    let destination = wide(destination);
    // SAFETY: both pointers are to null-terminated UTF-16 paths valid for the call.
    // The files are in one directory; do not allow a non-atomic copy fallback.
    let moved = unsafe {
        MoveFileExW(
            stage.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::client) struct TestState(pub(in crate::client) PathBuf);

#[cfg(test)]
impl TestState {
    pub(in crate::client) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let root = env::temp_dir().join(format!(
                "corral-app-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Self(root),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create Windows test state: {error}"),
            }
        }
    }
}

#[cfg(test)]
impl Drop for TestState {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

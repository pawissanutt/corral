//! A native download: the platform's save dialog where it has one, else the Downloads folder
//! under a name that replaces nothing. Bytes go to a private temporary file in the destination's
//! folder, which only a verified, complete read renames into place; dropped, it is deleted.
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

use nsh_core::file_dialog::SaveDialog;
use nsh_core::prelude::*;
use tempfile::NamedTempFile;

use crate::client::download::{SaveFinish, SaveOutput, SaveSink};
use crate::model::Form;

/// Numbered names tried in the Downloads folder before giving up.
const NUMBERED: u32 = 100;

pub(super) async fn sink(name: &str, form: Form) -> Result<Option<SaveOutput>, String> {
    // A terminal has no dialog of its own; the desktop asks where.
    if form != Form::Terminal {
        let dialog = SaveDialog::new()
            .file_name(name)
            .title("Save artifact")
            .button_label("Save");
        match pick_save_path(dialog).await {
            Ok(Some(path)) => return NativeSave::chosen(path).map(|save| Some(boxed(save))),
            Ok(None) => return Ok(None),
            // No dialog on this platform: the Downloads folder instead.
            Err(_) => {}
        }
    }
    NativeSave::downloads(name).map(|save| Some(boxed(save)))
}

fn boxed(save: NativeSave) -> SaveOutput {
    Box::new(save)
}

pub(super) struct NativeSave {
    file: NamedTempFile,
    /// The path chosen in the dialog, and whether a file stood there when the owner agreed to
    /// replace it; else none, and the file takes the first free name in `folder`.
    chosen: Option<(PathBuf, bool)>,
    folder: PathBuf,
    name: String,
}

impl NativeSave {
    fn chosen(path: PathBuf) -> Result<Self, String> {
        let folder = path
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
            .ok_or("The chosen path has no folder.")?
            .to_owned();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or("The chosen path has no file name.")?;
        // The dialog asked before replacing a file there now, and only that one.
        let replace = path.exists();
        Self::open(folder, name, Some((path, replace)))
    }

    fn downloads(name: &str) -> Result<Self, String> {
        Self::open(downloads_folder()?, name.to_owned(), None)
    }

    fn open(
        folder: PathBuf,
        name: String,
        chosen: Option<(PathBuf, bool)>,
    ) -> Result<Self, String> {
        let file = tempfile::Builder::new()
            .prefix(".corral-download-")
            .tempfile_in(&folder)
            .map_err(|error| format!("Could not write in {}: {error}", folder.display()))?;
        Ok(Self {
            file,
            chosen,
            folder,
            name,
        })
    }

    fn place(mut self) -> Result<String, String> {
        self.file
            .flush()
            .and_then(|()| self.file.as_file().sync_all())
            .map_err(|error| format!("Could not write the file: {error}"))?;
        if let Some((path, replace)) = self.chosen {
            // A file that appeared at a new path during the download was not agreed to.
            let placed = if replace {
                self.file
                    .persist(&path)
                    .map(drop)
                    .map_err(|error| error.error)
            } else {
                self.file
                    .persist_noclobber(&path)
                    .map(drop)
                    .map_err(|error| error.error)
            };
            placed.map_err(|error| format!("Could not save {}: {error}", path.display()))?;
            return Ok(format!("to {}", path.display()));
        }
        let mut file = self.file;
        for at in 0..=NUMBERED {
            let path = self.folder.join(numbered(&self.name, at));
            match file.persist_noclobber(&path) {
                Ok(_) => return Ok(format!("to {}", path.display())),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    file = error.file;
                }
                Err(error) => {
                    return Err(format!(
                        "Could not save {}: {}",
                        path.display(),
                        error.error
                    ));
                }
            }
        }
        Err(format!(
            "{} already holds {NUMBERED} files named like {}.",
            self.folder.display(),
            self.name
        ))
    }
}

impl SaveSink for NativeSave {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.file
            .write_all(bytes)
            .map_err(|error| format!("Could not write the file: {error}"))
    }

    fn finish(self: Box<Self>) -> SaveFinish {
        Box::pin(async move {
            blocking(move || self.place())
                .await
                .map_err(|error| error.to_string())?
        })
    }
}

/// `name`, or for `at` above zero `name (at)` before its extension.
fn numbered(name: &str, at: u32) -> String {
    if at == 0 {
        return name.to_owned();
    }
    match name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) {
        Some((stem, extension)) => format!("{stem} ({at}).{extension}"),
        None => format!("{name} ({at})"),
    }
}

/// The owner's Downloads folder: `XDG_DOWNLOAD_DIR`, else `Downloads` in their home, else home.
fn downloads_folder() -> Result<PathBuf, String> {
    if let Some(folder) = std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from)
        && folder.is_absolute()
        && folder.is_dir()
    {
        return Ok(folder);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or("No home folder to save in.")?;
    let downloads = home.join("Downloads");
    Ok(if Path::new(&downloads).is_dir() {
        downloads
    } else {
        home
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_never_replaces_a_file_it_was_not_told_to_and_leaves_none_when_dropped() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("report.md"), b"kept").unwrap();
        let mut save = NativeSave::open(folder.path().into(), "report.md".into(), None).unwrap();
        save.append(b"new bytes").unwrap();
        let at = save.place().unwrap();
        assert!(at.ends_with("report (1).md"));
        assert_eq!(
            std::fs::read(folder.path().join("report.md")).unwrap(),
            b"kept"
        );
        assert_eq!(
            std::fs::read(folder.path().join("report (1).md")).unwrap(),
            b"new bytes"
        );

        let mut dropped = NativeSave::open(folder.path().into(), "x.bin".into(), None).unwrap();
        dropped.append(b"partial").unwrap();
        drop(dropped);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    }
}

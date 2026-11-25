//! A native download: the platform's save dialog where it has one, else the Downloads folder.
//! It never replaces a file: what stands at the chosen name (whether the dialog showed it or it
//! appeared during the download) is kept and the file takes the first free numbered name. Bytes
//! go to a private temporary file in the destination's
//! folder, which only a verified, complete read renames into place; dropped, it is deleted.
use std::{
    ffi::{OsStr, OsString},
    io::Write as _,
    path::{Path, PathBuf},
};

use nsh_core::file_dialog::SaveDialog;
use nsh_core::prelude::*;
use tempfile::NamedTempFile;

use crate::client::download::{SaveFinish, SaveOutput, SaveSink, SaveStep};
use crate::model::Form;

/// Numbered names tried beside a taken one before giving up.
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
    folder: PathBuf,
    /// As the platform names it: a chosen name that is not UTF-8 is kept as it is.
    name: OsString,
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
            .map(OsStr::to_owned)
            .ok_or("The chosen path has no file name.")?;
        Self::open(folder, name)
    }

    fn downloads(name: &str) -> Result<Self, String> {
        Self::open(downloads_folder()?, name.into())
    }

    fn open(folder: PathBuf, name: OsString) -> Result<Self, String> {
        let file = tempfile::Builder::new()
            .prefix(".corral-download-")
            .tempfile_in(&folder)
            .map_err(|error| format!("Could not write in {}: {error}", folder.display()))?;
        Ok(Self { file, folder, name })
    }

    fn place(mut self) -> Result<String, String> {
        self.file
            .flush()
            .and_then(|()| self.file.as_file().sync_all())
            .map_err(|error| format!("Could not write the file: {error}"))?;
        let mut file = self.file;
        for at in 0..=NUMBERED {
            let path = self.folder.join(numbered(&self.name, at));
            match file.persist_noclobber(&path) {
                Ok(_) if at == 0 => return Ok(format!("to {}", path.display())),
                Ok(_) => {
                    return Ok(format!(
                        "to {}, since {} already exists and was kept",
                        path.display(),
                        self.name.to_string_lossy()
                    ));
                }
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
            self.name.to_string_lossy()
        ))
    }
}

impl NativeSave {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.file
            .write_all(bytes)
            .map_err(|error| format!("Could not write the file: {error}"))
    }
}

impl SaveSink for NativeSave {
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> SaveStep<'a> {
        Box::pin(std::future::ready(self.write(bytes)))
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
fn numbered(name: &OsStr, at: u32) -> OsString {
    if at == 0 {
        return name.to_owned();
    }
    let Some(name) = name.to_str() else {
        let mut name = name.to_owned();
        name.push(format!(" ({at})"));
        return name;
    };
    let named = match name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) {
        Some((stem, extension)) => format!("{stem} ({at}).{extension}"),
        None => format!("{name} ({at})"),
    };
    named.into()
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

    /// A path the dialog answered while it was free, taken during the download, is kept: the
    /// owner never agreed to replace it. A dropped download leaves no file.
    #[test]
    fn a_chosen_path_taken_during_the_download_is_kept_and_a_dropped_one_leaves_nothing() {
        let folder = tempfile::tempdir().unwrap();
        let chosen = folder.path().join("report.md");
        let mut save = NativeSave::chosen(chosen.clone()).unwrap();
        save.write(b"new bytes").unwrap();
        std::fs::write(&chosen, b"appeared meanwhile").unwrap();
        let at = save.place().unwrap();
        assert!(at.contains("report (1).md"), "{at}");
        assert_eq!(std::fs::read(&chosen).unwrap(), b"appeared meanwhile");
        assert_eq!(
            std::fs::read(folder.path().join("report (1).md")).unwrap(),
            b"new bytes"
        );

        let mut dropped = NativeSave::open(folder.path().into(), "x.bin".into()).unwrap();
        dropped.write(b"partial").unwrap();
        drop(dropped);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    }
}

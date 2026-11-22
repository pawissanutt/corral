//! The native browser sees only a private, temporary sandbox wrapper, not a worktree file URL.
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use nsh_core::prelude::blocking;
use tempfile::NamedTempFile;

use crate::client::html::{HtmlFinish, HtmlSink};

use super::{PREFIX, SUFFIX, Slot, escape};

pub(super) struct NativePreview {
    file: NamedTempFile,
    slot: Slot,
}

impl NativePreview {
    pub(super) fn new() -> Result<Self, String> {
        let slot = Slot::reserve()?;
        let mut file = tempfile::Builder::new()
            .prefix("corral-preview-")
            .suffix(".html")
            .tempfile()
            .map_err(|error| error.to_string())?;
        file.write_all(PREFIX).map_err(|error| error.to_string())?;
        Ok(Self { file, slot })
    }
}

impl HtmlSink for NativePreview {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.file
            .write_all(&escape(bytes))
            .map_err(|error| error.to_string())
    }

    fn finish(self: Box<Self>) -> HtmlFinish {
        Box::pin(async move {
            blocking(move || self.launch())
                .await
                .map_err(|error| error.to_string())?
        })
    }
}

impl NativePreview {
    fn launch(self) -> Result<(), String> {
        let Self { mut file, slot } = self;
        file.write_all(SUFFIX)
            .and_then(|()| file.flush())
            .map_err(|error| error.to_string())?;
        let launcher = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut child = Command::new(launcher)
            .arg(file.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;

        // Catch ordinary launcher refusals without waiting for a newly started browser to exit.
        let started = Instant::now();
        let mut exited = false;
        while started.elapsed() < Duration::from_millis(300) {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                if !status.success() {
                    return Err(format!("{launcher} failed ({status})."));
                }
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // A successful launcher may return before the browser reads the file. Conversely,
        // xdg-open may wait for the browser, which must not keep the snapshot indefinitely.
        std::thread::Builder::new()
            .name("html-preview".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(60).saturating_sub(started.elapsed()));
                drop(file);
                if !exited {
                    let _ = child.wait();
                }
                drop(slot);
            })
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

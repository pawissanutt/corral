//! Open an explicitly requested snapshot, never a file URL into the worktree.
use nsh_core::prelude::*;

use super::{AppState, ButtonKind, button, note};

pub(super) fn browser_button(app: AppState, source: &str, current: bool) -> Option<AnyElement> {
    if !current {
        return None;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (app, source);
        Some(note(
            "Open this preview in a web session to launch it in a browser.",
        ))
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let source = source.to_owned();
        Some(view().flex_col()
            .child(note("Opens a temporary sandboxed copy. Inline scripts can run and may navigate the frame to external sites. Open only files you trust."))
            .child(button(app, "Open in browser", ButtonKind::Ghost, "btn-open-html-browser", move || {
                if let Err(error) = open_copy(&source) {
                    app.toast(format!("Could not launch the browser: {error}"));
                }
            })).into_element())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_copy(source: &str) -> std::io::Result<()> {
    use std::{
        io::Write,
        process::{Command, Stdio},
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    static OPEN: AtomicUsize = AtomicUsize::new(0);
    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            OPEN.fetch_sub(1, Ordering::Relaxed);
        }
    }
    OPEN.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        (count < 4).then_some(count + 1)
    })
    .map_err(|_| std::io::Error::other("four browser previews are already opening"))?;
    let slot = Slot;
    let mut file = tempfile::Builder::new()
        .prefix("corral-preview-")
        .suffix(".html")
        .tempfile()?;
    file.write_all(super::sandboxed_document(source).as_bytes())?;
    file.flush()?;
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
        .spawn()?;
    // A successful launcher can return before the browser has read the snapshot.
    std::thread::Builder::new()
        .name("html-preview".into())
        .spawn(move || {
            let _slot = slot;
            let _ = child.wait();
            std::thread::sleep(Duration::from_secs(60));
            drop(file);
        })?;
    Ok(())
}

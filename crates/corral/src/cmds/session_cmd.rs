use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_ops::Ctx;

use super::start_dir;

pub(super) fn hook(root: &Option<PathBuf>, entry: &crate::entry::Entry) {
    let result = start_dir(root).and_then(|start| {
        if nudge_hook(&start)? {
            return Ok(());
        }
        let ctx = Ctx::open_for_sync(&start)?;
        entry.apply(&ctx.store)?;
        coalesced_sync(&ctx)
    });
    if let Err(e) = result {
        eprintln!("corral hook: {e:#}");
    }
}

fn nudge_hook(start: &Path) -> Result<bool> {
    let root = corral_core::Store::find_root(start)?;
    Ok(corral_ops::daemon_client::nudge_at(
        &root.join(".corral"),
        "hook",
    ))
}

/// herdr starts one hook per pane event, and each ran its own full sync pass, so bursts piled
/// passes on top of each other. One hook syncs at a time: a hook that finds one running marks
/// the store dirty and leaves, and the running hook makes one more pass for those marks.
/// Marking then retrying the lock once, and re-checking the mark after unlocking, leaves no
/// window in which a recorded event waits for a later event. The trailing pass is capped so a
/// continuous burst cannot keep one hook process alive; a mark left after it is taken by the
/// next hook or the board's periodic sync.
fn coalesced_sync(ctx: &Ctx) -> Result<()> {
    let dir = ctx.store.cache_root()?;
    std::fs::create_dir_all(&dir).context("creating hook sync state directory")?;
    let dirty = dir.join("hook-sync.dirty");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("hook-sync.lock"))
        .context("opening hook sync lock")?;
    for _ in 0..2 {
        if !try_lock(&lock)? {
            std::fs::write(&dirty, b"").context("marking hook sync dirty")?;
            if !try_lock(&lock)? {
                return Ok(());
            }
        }
        match std::fs::remove_file(&dirty) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("clearing hook sync mark"),
        }
        corral_ops::sync_hook(ctx)?;
        lock.unlock().context("unlocking hook sync")?;
        if !dirty.exists() {
            return Ok(());
        }
    }
    Ok(())
}

fn try_lock(lock: &std::fs::File) -> Result<bool> {
    match lock.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e).context("locking hook sync"),
    }
}

pub(super) fn bind(
    root: &Option<PathBuf>,
    socket: &Path,
    launch_record: &Path,
    entry: &crate::entry::Entry,
) -> Result<()> {
    let ctx = Ctx::open_for_sync(&start_dir(root)?)?;
    entry.apply(&ctx.store)?;
    let evidence = std::fs::read_to_string(launch_record)
        .with_context(|| format!("reading {}", launch_record.display()))?;
    ctx.store.bind_herdr_session(socket, &evidence)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use corral_protocol::frame::{Frame, FrameKind, read_frame, write_frame};
    use serde_json::json;

    #[test]
    fn answered_hook_never_opens_store_or_creates_coalescing_lock() {
        let dir = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(dir.path()).unwrap();
        let config = std::fs::read(dir.path().join(".corral/corral.toml")).unwrap();
        let cache = store.cache_root().unwrap();
        let socket = corral_ops::daemon_client::socket_path(&store);
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        drop(store);
        // A store open would fail on this config; a socket-only hook must still answer.
        std::fs::write(dir.path().join(".corral/corral.toml"), "invalid = [").unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            for method in [corral_protocol::HELLO, corral_protocol::NUDGE] {
                let frame = read_frame(&mut stream).unwrap().unwrap();
                let request: corral_protocol::rpc::Request =
                    serde_json::from_slice(&frame.payload).unwrap();
                assert_eq!(request.method, method);
                let result = if method == corral_protocol::HELLO {
                    json!({"proto":{"major":1,"minor":0},"caps":[],"daemon":{"version":"test","store":"test","epoch":1},"health":corral_protocol::Health::down()})
                } else {
                    assert_eq!(request.params["source"], "hook");
                    json!({"pass_seq":1,"completed":false})
                };
                write_frame(
                    &mut stream,
                    &Frame {
                        kind: FrameKind::Control,
                        stream: 0,
                        payload: serde_json::to_vec(
                            &json!({"jsonrpc":"2.0","id":request.id,"result":result}),
                        )
                        .unwrap(),
                    },
                )
                .unwrap();
            }
        });
        let matches = crate::Cli::command().get_matches_from(["corral", "hook"]);
        let entry = crate::entry::Entry::new(&crate::Cmd::Hook, &matches, dir.path()).unwrap();
        hook(&Some(dir.path().to_path_buf()), &entry);
        server.join().unwrap();
        assert!(!cache.join("hook-sync.lock").exists());
        // An absent listener chooses the legacy path rather than claiming success.
        std::fs::remove_file(&socket).unwrap();
        assert!(!nudge_hook(dir.path()).unwrap());
        std::fs::write(dir.path().join(".corral/corral.toml"), config).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(cache.join("hook-sync.lock"))
            .unwrap();
        lock.lock().unwrap();
        hook(&Some(dir.path().to_path_buf()), &entry);
        assert!(cache.join("hook-sync.dirty").exists());
        lock.unlock().unwrap();
        std::fs::remove_dir(socket.parent().unwrap()).unwrap();
    }
}

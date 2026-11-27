//! A Git read whose output and run time are both capped: past `max_bytes` the reader sees end
//! of input and the run is reported as exceeded; past `deadline` the process is killed and the
//! read fails. Either way the child is killed and reaped, never left running.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, atomic::AtomicBool, atomic::Ordering, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::{cmdline, inherit_command_leases, scrub_env, work};

pub(crate) struct Limits<'a> {
    pub env: &'a [(&'a str, &'a str)],
    /// Written to the child's stdin, then closed.
    pub input: &'a [u8],
    pub max_bytes: u64,
    pub deadline: Duration,
}

/// What `read` returned, and whether the output was cut at `max_bytes`.
pub(crate) struct Bounded<T> {
    pub value: T,
    pub exceeded: bool,
}

/// Run `git -C repo args` and hand its stdout, capped at `limits.max_bytes`, to `read`.
/// A complete run must exit successfully; a cut or early-stopped one is killed.
pub(crate) fn read_bounded<T>(
    repo: &Path,
    args: &[&str],
    limits: &Limits<'_>,
    read: impl FnOnce(&mut dyn BufRead) -> Result<T>,
) -> Result<Bounded<T>> {
    work::record();
    let _timer = work::timed();
    let mut command = Command::new("git");
    scrub_env(&mut command, std::env::vars_os().map(|(name, _)| name));
    let _leases = inherit_command_leases(&mut command)?;
    let mut child = command
        .envs(limits.env.iter().copied())
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn `{}`", cmdline(repo, args)))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let child = Arc::new(Mutex::new(child));
    let fired = Arc::new(AtomicBool::new(false));
    let (done, wait) = mpsc::channel::<()>();
    let watchdog = {
        let (child, fired, deadline) = (Arc::clone(&child), Arc::clone(&fired), limits.deadline);
        thread::spawn(move || {
            if wait.recv_timeout(deadline) == Err(mpsc::RecvTimeoutError::Timeout) {
                fired.store(true, Ordering::SeqCst);
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        })
    };
    let result = (|| {
        if let Some(mut stdin) = stdin {
            // Small inputs (a page of object IDs) fit the pipe before Git reads any.
            stdin.write_all(limits.input).context("write git stdin")?;
        }
        let stdout = stdout.context("git stdout")?;
        let mut reader = BufReader::new(stdout.take(limits.max_bytes));
        let value = read(&mut reader)?;
        // Unread output means the reader stopped early (within the cap) or the cap cut it.
        let unread = !reader.fill_buf().context("read git stdout")?.is_empty();
        let mut probe = [0u8; 1];
        let exceeded = !unread
            && reader.get_ref().limit() == 0
            && reader.get_mut().get_mut().read(&mut probe).unwrap_or(0) > 0;
        Ok::<_, anyhow::Error>((value, unread, exceeded))
    })();
    let _ = done.send(());
    let _ = watchdog.join();
    let complete = matches!(result, Ok((_, false, false)));
    let status = {
        let mut child = child
            .lock()
            .map_err(|_| anyhow::anyhow!("git child lock poisoned"))?;
        finish(&mut child, !complete)?
    };
    if fired.load(Ordering::SeqCst) {
        bail!(
            "`{}` took longer than {:?}",
            cmdline(repo, args),
            limits.deadline
        );
    }
    let (value, _, exceeded) = result?;
    if complete && !status {
        bail!("`{}` failed", cmdline(repo, args));
    }
    Ok(Bounded { value, exceeded })
}

/// Kill the child when its output is no longer wanted, then reap it; true when it succeeded.
fn finish(child: &mut Child, kill: bool) -> Result<bool> {
    if kill {
        let _ = child.kill();
    }
    Ok(child.wait().context("wait for git")?.success())
}

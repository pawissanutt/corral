//! Bounded subprocess exchange for owner-trusted extension lines. This is not a sandbox.

use std::{collections::BTreeMap, path::Path, time::Duration};

use anyhow::Result;
use serde_json::{Map, Value};

pub struct CommandContext<'a> {
    pub root: &'a Path,
    pub cwd: &'a Path,
    pub project: Option<&'a str>,
    pub extension: &'a str,
    pub env: &'a BTreeMap<String, String>,
}

/// Run an owner-configured `sh -c` line with a versioned JSON request on stdin.
/// Returns raw stdout on success; on failure only the stderr tail is diagnostic.
/// The runner must exclusively own child reaping (no host-wide SIGCHLD reaper).
/// On timeout it signals the process group, then waits to reap the shell.
#[cfg(unix)]
pub fn run(
    line: &str,
    request: Map<String, Value>,
    context: &CommandContext<'_>,
    timeout: Duration,
    max_stdout: usize,
) -> Result<Vec<u8>> {
    unix::run(line, request, context, timeout, max_stdout)
}

#[cfg(not(unix))]
pub fn run(
    _line: &str,
    _request: Map<String, Value>,
    _context: &CommandContext<'_>,
    _timeout: Duration,
    _max_stdout: usize,
) -> Result<Vec<u8>> {
    anyhow::bail!("extension commands are supported only on Unix")
}

#[cfg(unix)]
mod unix {
    use std::{
        io::{self, Read, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
        time::Instant,
    };

    use anyhow::{Context, anyhow, bail, ensure};

    use super::{CommandContext, Duration, Map, Result, Value};

    const STDERR_LIMIT: usize = 4 * 1024;
    const CHUNK: usize = 16 * 1024;
    // A shell can exit without generating a pipe event (e.g. its descendants hold
    // the descriptors). Check its status independently of poll readiness.
    const STATUS_INTERVAL: Duration = Duration::from_millis(25);

    pub(super) fn run(
        line: &str,
        mut request: Map<String, Value>,
        context: &CommandContext<'_>,
        timeout: Duration,
        max_stdout: usize,
    ) -> Result<Vec<u8>> {
        request.insert("v".into(), Value::from(1));
        let input = serde_json::to_vec(&request).context("encode extension request")?;
        let started = Instant::now();
        let mut child = Command::new("sh")
            .args(["-c", line])
            .current_dir(context.cwd)
            .envs(context.env)
            .env("CORRAL_NEXT_ROOT", context.root)
            .env("CORRAL_PROJECT", context.project.unwrap_or_default())
            .env("CORRAL_EXTENSION", context.extension)
            .env("CORRAL_ACTOR", "extension")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("start extension command")?;
        // A Unix Child id originates as a positive pid_t from spawn.
        let pid = child.id() as libc::pid_t;
        let mut pipes = Pipes {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
        };
        let mut capture = Capture::default();
        let setup = pipes.nonblocking();
        let can_drain = setup.is_ok();
        let result = setup
            .and_then(|()| pipes.exchange(pid, &input, started, timeout, max_stdout, &mut capture));
        let Err(error) = result else {
            // exchange observed a successful exit without reaping it.
            child.wait().context("reap extension shell")?;
            return Ok(capture.stdout);
        };

        // Never reap before killing the group: the unreaped shell pins the
        // numeric group ID even if the shell exited ahead of its descendants.
        drop(pipes.stdin.take());
        let cleanup = stop(&mut child, pid);
        let tail = &mut capture.stderr_tail;
        if can_drain && let Some(stderr) = &mut pipes.stderr {
            drain_queued_stderr(stderr, tail);
        }
        if tail.is_empty() && cleanup.is_ok() {
            return Err(error);
        }
        let mut message = format!("{error:#}");
        if let Err(cleanup) = cleanup {
            message.push_str(&format!("; cleanup failed: {cleanup:#}"));
        }
        if !tail.is_empty() {
            message.push_str("\nstderr tail:\n");
            message.push_str(&String::from_utf8_lossy(tail));
        }
        Err(anyhow!(message))
    }

    #[derive(Default)]
    struct Capture {
        stdout: Vec<u8>,
        stderr_tail: Vec<u8>,
    }

    struct Pipes {
        stdin: Option<ChildStdin>,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    }

    impl Pipes {
        fn nonblocking(&self) -> Result<()> {
            for (fd, name) in [
                (fd(&self.stdin), "stdin"),
                (fd(&self.stdout), "stdout"),
                (fd(&self.stderr), "stderr"),
            ] {
                set_nonblocking(fd)
                    .with_context(|| format!("set extension {name} to nonblocking"))?;
            }
            Ok(())
        }

        fn exchange(
            &mut self,
            pid: libc::pid_t,
            mut input: &[u8],
            started: Instant,
            timeout: Duration,
            max_stdout: usize,
            capture: &mut Capture,
        ) -> Result<()> {
            let mut shell_exited = false;
            let mut buf = [0; CHUNK];
            loop {
                let remaining = timeout.saturating_sub(started.elapsed());
                ensure!(
                    !remaining.is_zero(),
                    "extension command timed out after {timeout:?}"
                );
                if !shell_exited {
                    shell_exited = shell_succeeded(pid)?;
                }
                if shell_exited
                    && self.stdin.is_none()
                    && self.stdout.is_none()
                    && self.stderr.is_none()
                {
                    return Ok(());
                }
                let mut fds = [
                    pollfd(&self.stdin, libc::POLLOUT),
                    pollfd(&self.stdout, libc::POLLIN),
                    pollfd(&self.stderr, libc::POLLIN),
                ];
                let delay = if shell_exited {
                    remaining
                } else {
                    remaining.min(STATUS_INTERVAL)
                };
                // Round up to avoid spinning when less than a millisecond remains.
                let millis = (delay.as_millis() + 1).min(libc::c_int::MAX as u128) as libc::c_int;
                // SAFETY: fds is a live array of pollfd values, with each nonnegative fd
                // owned by self. poll takes the array length and does not retain pointers.
                if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) } < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error).context("poll extension command pipes");
                }
                // Process at most one nonblocking chunk per fd to give the other
                // pipes, the shell status, and the deadline a turn.
                if let Some(stderr) = &mut self.stderr
                    && fds[2].revents != 0
                {
                    match retryable(stderr.read(&mut buf)).context("read extension stderr")? {
                        Some(0) => self.stderr = None,
                        Some(n) => keep_tail(&mut capture.stderr_tail, &buf[..n]),
                        None => {}
                    }
                }
                if let Some(stdout) = &mut self.stdout
                    && fds[1].revents != 0
                {
                    match retryable(stdout.read(&mut buf)).context("read extension stdout")? {
                        Some(0) => self.stdout = None,
                        Some(n) => {
                            ensure!(
                                n <= max_stdout - capture.stdout.len(),
                                "extension stdout exceeded {max_stdout} bytes"
                            );
                            capture.stdout.extend_from_slice(&buf[..n]);
                        }
                        None => {}
                    }
                }
                if let Some(stdin) = &mut self.stdin
                    && fds[0].revents != 0
                {
                    let chunk = &input[..input.len().min(CHUNK)];
                    match retryable(stdin.write(chunk)) {
                        Ok(Some(0)) => bail!("writing extension request to stdin made no progress"),
                        Ok(Some(n)) => {
                            input = &input[n..];
                            if input.is_empty() {
                                self.stdin = None; // EOF immediately after the JSON object
                            }
                        }
                        Ok(None) => {}
                        // An extension may opt not to read the request. Its
                        // exit status, not the closed pipe, determines success.
                        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                            self.stdin = None
                        }
                        Err(error) => {
                            return Err(error).context("write extension request to stdin");
                        }
                    }
                }
            }
        }
    }

    fn fd(pipe: &Option<impl AsRawFd>) -> libc::c_int {
        pipe.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }

    fn pollfd(pipe: &Option<impl AsRawFd>, events: libc::c_short) -> libc::pollfd {
        libc::pollfd {
            fd: fd(pipe),
            events,
            revents: 0,
        }
    }

    fn set_nonblocking(fd: libc::c_int) -> io::Result<()> {
        // SAFETY: fcntl operates on a live, owned fd and these flags take no pointer.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn retryable(result: io::Result<usize>) -> io::Result<Option<usize>> {
        match result {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            _ => result.map(Some),
        }
    }

    fn peek_exit(pid: libc::pid_t) -> io::Result<libc::siginfo_t> {
        // SAFETY: zeroing makes a valid empty siginfo_t. waitid writes into it if
        // there is exit status for this child; WNOWAIT leaves the PID reserved.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } == -1
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    fn shell_succeeded(pid: libc::pid_t) -> Result<bool> {
        let info = match peek_exit(pid) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(false),
            result => result.context("inspect extension shell status")?,
        };
        // SAFETY: these fields are used with the P_PID/WEXITED waitid result above.
        if unsafe { info.si_pid() } == 0 {
            return Ok(false);
        }
        let status = unsafe { info.si_status() };
        if info.si_code != libc::CLD_EXITED {
            bail!("extension shell killed by signal {status}");
        }
        ensure!(status == 0, "extension shell exited with status {status}");
        Ok(true)
    }

    fn stop(child: &mut Child, pid: libc::pid_t) -> Result<()> {
        // Confirm that our unreaped child still pins this ID. Do not signal a
        // numeric ID if waitid cannot establish ownership.
        loop {
            match peek_exit(pid) {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(error)
                        .context("verify ownership of extension shell before cleanup");
                }
            }
        }
        let group = kill(-pid).context("kill extension process group");
        // Also target the direct child in case it changed process group itself.
        let reaped = match kill(pid) {
            Ok(()) => child.wait().map(|_| ()).context("reap extension shell"),
            Err(error) => {
                // Reap if it already exited, but don't block on a child that
                // couldn't be signaled. The group has already been signaled.
                if child.try_wait().context("reap extension shell")?.is_some() {
                    Ok(())
                } else {
                    Err(error).context("kill extension shell")
                }
            }
        };
        group.and(reaped)
    }

    fn kill(target: libc::pid_t) -> io::Result<()> {
        // SAFETY: target is our unreaped child's PID (positive), or its negation
        // to signal the group created for this child.
        if unsafe { libc::kill(target, libc::SIGKILL) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn keep_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
        let bytes = &bytes[bytes.len().saturating_sub(STDERR_LIMIT)..];
        let excess = (tail.len() + bytes.len()).saturating_sub(STDERR_LIMIT);
        tail.drain(..excess);
        tail.extend_from_slice(bytes);
    }

    fn drain_queued_stderr(stderr: &mut ChildStderr, tail: &mut Vec<u8>) {
        // Snapshot the queued bytes so error reporting cannot be prolonged by a
        // descendant that escaped the group and keeps the pipe open or writes.
        let mut queued: libc::c_int = 0;
        // SAFETY: stderr is live, FIONREAD takes a pointer to an int.
        if unsafe { libc::ioctl(stderr.as_raw_fd(), libc::FIONREAD, &mut queued) } == -1 {
            return;
        }
        let mut remaining = queued.max(0) as usize;
        let mut buf = [0; CHUNK];
        while remaining != 0 {
            match stderr.read(&mut buf[..remaining.min(CHUNK)]) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    keep_tail(tail, &buf[..n]);
                    remaining -= n;
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
#[path = "extension_tests.rs"]
mod tests;

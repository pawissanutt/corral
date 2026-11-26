use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::{LaunchSpec, Origin, Runner, SessionRef, SessionStatus};
use crate::{Next, Op, error::refuse};

/// Detached shell commands. Each launch gets a new Unix session/process group.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRunner;

// Arguments, rather than interpolation, preserve arbitrary command and directory bytes.
// The supervising shell remains alive to record completion even after corral exits.
const SUPERVISOR: &str = r#"
trap 'code=$?; trap - EXIT; printf "%s\n" "$code" > "$2/.exit_code.$$" && mv -f "$2/.exit_code.$$" "$2/exit_code"' EXIT
trap 'exit 143' TERM
trap '' HUP
sh -c "$1"
"#;

impl Runner for ProcessRunner {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef> {
        fs::create_dir_all(&spec.dir)
            .with_context(|| format!("create process run directory {}", spec.dir.display()))?;
        let dir = fs::canonicalize(&spec.dir)
            .with_context(|| format!("resolve process run directory {}", spec.dir.display()))?;
        if completed(&dir)? {
            return Err(retry_refusal(
                &spec.dir,
                "process run directory already has an exit_code",
                None,
            ));
        }
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(SUPERVISOR)
            .arg("corral-process")
            .arg(&spec.command)
            .arg(&dir)
            .current_dir(&spec.cwd)
            .envs(spec.env.iter().map(|(key, value)| (key, value)));
        spawn(command, dir).with_context(|| format!("launch process session {}", spec.label))
    }

    fn start_process(&self, command: Command, dir: PathBuf) -> Result<SessionRef> {
        // Unlike task runs, a chat relaunches in the same placement directory.
        let marker = dir.join("exit_code");
        match fs::remove_file(&marker) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("clear prior process exit marker"),
        }
        spawn(command, dir)
    }

    fn status(&self, session: &SessionRef) -> Result<SessionStatus> {
        let (pid, dir) = process_session(session)?;
        Ok(if completed(dir)? || !placement_alive(pid)? {
            SessionStatus::Gone
        } else {
            SessionStatus::Working
        })
    }

    #[cfg(unix)]
    fn checkout_session_gone(&self, session: &SessionRef) -> Result<bool> {
        let (pid, _) = process_session(session)?;
        let pid = libc::pid_t::try_from(pid).context("process group exceeds Unix pid range")?;
        if pid <= 1 {
            bail!("invalid process session group {pid}");
        }
        // A leader can record completion or exit while a child is still alive.
        Ok(!group_exists(pid)?)
    }

    fn send(&self, session: &SessionRef, text: &str, _origin: Origin) -> Result<()> {
        let (_, dir) = process_session(session)?;
        Err(retry_refusal(
            dir,
            "headless process sessions cannot receive messages",
            Some(text),
        ))
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        let (pid, dir) = process_session(session)?;
        if completed(dir)? {
            return Ok(());
        }
        stop_group(pid, dir)
    }

    fn focus(&self, session: &SessionRef) -> Result<()> {
        let (_, dir) = process_session(session)?;
        let id = task_id(dir).unwrap_or("<task>");
        Err(refuse(
            format!("headless process sessions have no pane to focus; use `corral show {id}`"),
            Vec::new(),
        ))
    }
}

fn spawn(mut command: Command, dir: PathBuf) -> Result<SessionRef> {
    let log_path = dir.join("session.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open process log {}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .with_context(|| format!("clone process log {}", log_path.display()))?;
    command.stdin(Stdio::null()).stdout(log).stderr(stderr);
    detach(&mut command)?;
    let mut child = command
        .spawn()
        .with_context(|| format!("start {:?} in {}", command.get_program(), dir.display()))?;
    let pid = child.id();
    // Reap while the caller is alive. Otherwise init inherits the detached host.
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(SessionRef::Process { pid, dir })
}

fn process_session(session: &SessionRef) -> Result<(u32, &Path)> {
    match session {
        SessionRef::Process { pid, dir } => Ok((*pid, dir)),
        SessionRef::Herdr { .. } | SessionRef::Pi { .. } => Err(refuse(
            "this session is not owned by the process runner; set runner.mode to \"herdr\" in .corral-next/corral.toml",
            Vec::new(),
        )),
    }
}

fn task_id(dir: &Path) -> Option<&str> {
    let id = dir.parent()?.file_name()?.to_str()?;
    let number = id.strip_prefix("T-")?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())).then_some(id)
}

fn retry_refusal(dir: &Path, reason: &str, note: Option<&str>) -> anyhow::Error {
    let Some(id) = task_id(dir) else {
        return refuse(format!("{reason}; use `corral retry <task>`"), Vec::new());
    };
    refuse(
        reason,
        vec![Next::run(Op::Retry {
            id: id.to_owned(),
            profile: None,
            fresh: false,
            note: note.map(str::to_owned),
        })],
    )
}

fn completed(dir: &Path) -> Result<bool> {
    dir.join("exit_code")
        .try_exists()
        .with_context(|| format!("inspect process completion marker in {}", dir.display()))
}

#[cfg(unix)]
fn placement_alive(pid: u32) -> Result<bool> {
    let pid = libc::pid_t::try_from(pid).context("process session pid exceeds Unix pid range")?;
    if pid <= 1 {
        bail!("invalid process session pid {pid}");
    }
    // SAFETY: getsid only inspects a positive PID. A detached group leader
    // also leads its Unix session; do not mistake another process for it.
    let sid = unsafe { libc::getsid(pid) };
    if sid != -1 {
        return Ok(sid == pid);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error).context("inspect detached process session")
    }
}

#[cfg(not(unix))]
fn placement_alive(_: u32) -> Result<bool> {
    bail!("detached process sessions require Unix")
}

fn record_stopped(dir: &Path, code: i32) -> Result<()> {
    let path = dir.join("exit_code");
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("create {}", path.display())),
    };
    writeln!(file, "{code}").with_context(|| format!("write {}", path.display()))
}

#[cfg(unix)]
pub(crate) fn detach(command: &mut Command) -> Result<()> {
    use std::os::unix::process::CommandExt;

    // SAFETY: setsid is async-signal-safe and this closure touches no shared state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn detach(_command: &mut Command) -> Result<()> {
    bail!("detached process sessions require Unix")
}

#[cfg(unix)]
fn stop_group(pid: u32, dir: &Path) -> Result<()> {
    let pid = libc::pid_t::try_from(pid).context("process session pid exceeds Unix pid range")?;
    if pid <= 1 {
        bail!("refusing to signal invalid process session pid {pid}");
    }
    // A supplied PID must be the detached session and group leader, not an
    // arbitrary process in another application's group.
    // SAFETY: these syscalls inspect a positive PID and do not dereference pointers.
    let group = unsafe { libc::getpgid(pid) };
    if group == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return record_stopped(dir, 143);
        }
        return Err(error).context("inspect process session group before stopping");
    }
    // SAFETY: getsid only inspects a positive PID.
    let session = unsafe { libc::getsid(pid) };
    if session == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return record_stopped(dir, 143);
        }
        return Err(error).context("inspect detached process session before stopping");
    }
    if group != pid || session != pid {
        return Err(retry_refusal(
            dir,
            "process is not the detached session's process-group leader",
            None,
        ));
    }
    // SAFETY: zero arguments mean this caller and its own PID as group.
    // A corral command invoked by the agent must survive closing that agent.
    if unsafe { libc::getpgrp() == pid && libc::setpgid(0, 0) == -1 } {
        return Err(io::Error::last_os_error())
            .context("leave process session group before stopping");
    }
    signal_group(pid, libc::SIGTERM)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while group_exists(pid)? {
        if Instant::now() >= deadline {
            signal_group(pid, libc::SIGKILL)?;
            return record_stopped(dir, 137);
        }
        thread::sleep(Duration::from_millis(20));
    }
    record_stopped(dir, 143)
}

#[cfg(unix)]
fn signal_group(pid: libc::pid_t, signal: libc::c_int) -> Result<()> {
    // SAFETY: pid was checked to be positive; negating it targets only its group.
    if unsafe { libc::kill(-pid, signal) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error)
                .with_context(|| format!("send signal {signal} to process group {pid}"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn group_exists(pid: libc::pid_t) -> Result<bool> {
    // SAFETY: signal zero only inspects the checked process group.
    if unsafe { libc::kill(-pid, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error).with_context(|| format!("inspect process group {pid}")),
    }
}

#[cfg(not(unix))]
fn stop_group(_pid: u32, _dir: &Path) -> Result<()> {
    bail!("stopping process sessions requires Unix")
}

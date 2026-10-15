//! Bounded argv execution, complete file-backed output, and observable installed identity.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::process::{Pid, PidfdFlags, Signal, kill_process_group, pidfd_open};
use sha2::{Digest, Sha256};

use crate::hooks::{HookAttempt, HookState, PostLandOperation, save};
use crate::{Ctx, ProcessControl, SystemProcesses};

pub(crate) fn binary(op: &PostLandOperation, i: usize) -> Result<PathBuf> {
    let identity = op.hooks[i]
        .hook
        .identity
        .as_ref()
        .context("identity not configured")?;
    let attempt = op.hooks[i].attempts.last().context("hook has no attempt")?;
    Ok(attempt.cwd.join(&identity.binary))
}

fn hash(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).with_context(|| format!("inspecting {}", path.display()))?;
    if !metadata.is_file() {
        bail!(
            "identity is not a regular executable file: {}",
            path.display()
        );
    }
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file
            .read(&mut buffer)
            .context("hashing installed executable")?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn command(argv: &[String], attempt: &HookAttempt, log: &File) -> Result<Command> {
    let (exe, args) = argv.split_first().context("empty hook argv")?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(&attempt.cwd)
        .env_clear()
        .envs(&attempt.env)
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("cloning hook stdout log")?)
        .stderr(log.try_clone().context("cloning hook stderr log")?)
        .process_group(0);
    Ok(cmd)
}

/// Wait on the kernel exit event, not timing-dependent polling.
fn wait(child: &mut std::process::Child, timeout: u32) -> Result<(std::process::ExitStatus, bool)> {
    let pid = Pid::from_raw(i32::try_from(child.id())?).context("invalid hook PID")?;
    let fd = pidfd_open(pid, PidfdFlags::empty()).context("opening hook pidfd")?;
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(u64::from(timeout));
    let timed_out = loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let time = Timespec {
            tv_sec: i64::try_from(remaining.as_secs())?,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        match poll(&mut fds, Some(&time)) {
            Ok(0) => break true,
            Ok(_) => break false,
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e).context("waiting for hook exit"),
        }
    };
    // A successful direct child must not leave commands continuing behind the receipt.
    match kill_process_group(pid, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(e) => return Err(e).context("stopping hook process group"),
    }
    Ok((child.wait().context("reaping hook process")?, timed_out))
}

pub(crate) fn still_running(attempt: &HookAttempt) -> Result<bool> {
    if let Some(pid) = attempt.pid
        && let Some(process) = SystemProcesses.inspect(pid)?
        && attempt.process_start.as_deref() == Some(&process.started)
    {
        return Ok(true);
    }
    // Covers interruption between spawn and PID persistence, and orphaned descendants.
    let needle = format!("CORRAL_HOOK_TOKEN={}", attempt.token);
    for entry in fs::read_dir("/proc").context("reading process inventory")? {
        let entry = entry.context("reading process entry")?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        match fs::read(entry.path().join("environ")) {
            Ok(bytes) if bytes.split(|b| *b == 0).any(|s| s == needle.as_bytes()) => {
                return Ok(true);
            }
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(e) => return Err(e).context("reading hook process environment"),
        }
    }
    Ok(false)
}

pub(crate) fn execute(ctx: &Ctx, op: &mut PostLandOperation, i: usize) -> Result<()> {
    let hook = op.hooks[i].hook.clone();
    let cwd = op
        .repo
        .join(&hook.cwd)
        .canonicalize()
        .context("resolving hook cwd")?;
    if !cwd.starts_with(
        op.repo
            .canonicalize()
            .context("resolving project checkout")?,
    ) {
        bail!("hook cwd escapes project checkout");
    }
    let number = u32::try_from(op.hooks[i].attempts.len())?
        .checked_add(1)
        .context("attempt overflow")?;
    let dir = ctx.store.state_dir().join("post-land").join(&op.key);
    fs::create_dir_all(&dir).context("creating hook log directory")?;
    let log_path = dir.join(format!("{}-{number}.log", hook.name));
    let mut log = File::create(&log_path).context("creating full hook log")?;
    writeln!(
        log,
        "argv: {:?}\ncwd: {}\nlanded SHA: {}",
        hook.argv,
        cwd.display(),
        op.landed_sha
    )
    .context("writing hook log header")?;
    let mut env = hook.env.clone();
    let token = format!("{}-{}-{number}", op.key, hook.name);
    env.insert("CORRAL_LANDED_SHA".into(), op.landed_sha.clone());
    env.insert("CORRAL_BUILD_SHA".into(), op.landed_sha.clone());
    env.insert("CORRAL_PROJECT".into(), op.project.clone());
    env.insert("CORRAL_HOOK_TOKEN".into(), token.clone());
    op.hooks[i].attempts.push(HookAttempt {
        number,
        started: Timestamp::now(),
        finished: None,
        token,
        pid: None,
        process_start: None,
        cwd,
        env,
        log: log_path,
        exit_code: None,
        signal: None,
        timed_out: false,
        error: None,
        backup: None,
        installed_sha256: None,
        identity_stdout: None,
    });
    if hook.identity.is_some() {
        let target = binary(op, i)?;
        match fs::metadata(&target) {
            Ok(meta) if meta.is_file() => {
                let backup = dir.join(format!("{}-{number}.prior", hook.name));
                fs::copy(&target, &backup).context("preserving prior usable executable")?;
                File::open(&backup)?
                    .sync_all()
                    .context("syncing prior binary backup")?;
                op.hooks[i]
                    .attempts
                    .last_mut()
                    .context("missing attempt")?
                    .backup = Some(backup);
            }
            Ok(_) => bail!("installed binary path is not a regular file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("inspecting prior executable"),
        }
    }
    op.hooks[i].state = HookState::Running;
    save(ctx, op)?;
    let attempt = op.hooks[i]
        .attempts
        .last()
        .context("missing hook attempt")?;
    let spawned = command(&hook.argv, attempt, &log)?.spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            op.hooks[i]
                .attempts
                .last_mut()
                .context("missing attempt")?
                .finished = Some(Timestamp::now());
            return Err(error).with_context(|| format!("spawning hook {:?}", hook.argv));
        }
    };
    let attempt = op.hooks[i].attempts.last_mut().context("missing attempt")?;
    attempt.pid = Some(child.id());
    attempt.process_start = SystemProcesses.inspect(child.id())?.map(|p| p.started);
    save(ctx, op)?;
    let (status, timed_out) = wait(&mut child, hook.timeout_secs)?;
    log.sync_all().context("syncing complete hook log")?;
    let attempt = op.hooks[i].attempts.last_mut().context("missing attempt")?;
    attempt.exit_code = status.code();
    attempt.signal = status.signal();
    attempt.timed_out = timed_out;
    attempt.finished = Some(Timestamp::now());
    if !status.success() || timed_out {
        bail!(
            "hook {:?} failed: {status}, timeout={timed_out}; log {}",
            hook.argv,
            attempt.log.display()
        );
    }
    if hook.identity.is_some() {
        observe_identity(op, i)?;
    }
    op.hooks[i].state = HookState::Succeeded;
    op.hooks[i].reason = None;
    save(ctx, op)
}

pub(crate) fn observe_identity(op: &mut PostLandOperation, i: usize) -> Result<()> {
    let identity = op.hooks[i]
        .hook
        .identity
        .clone()
        .context("identity not configured")?;
    let target = binary(op, i)?;
    let fingerprint = hash(&target)?;
    let attempt = op.hooks[i].attempts.last().context("missing attempt")?;
    let probe_path = attempt.log.with_extension("identity.log");
    let log = File::create(&probe_path).context("creating identity probe log")?;
    let mut full = OpenOptions::new()
        .append(true)
        .open(&attempt.log)
        .context("opening hook log")?;
    let mut argv = vec![target.to_string_lossy().into_owned()];
    argv.extend(identity.args);
    let mut child = command(&argv, attempt, &log)?
        .stderr(full.try_clone().context("cloning identity stderr log")?)
        .spawn()
        .context("spawning identity probe")?;
    let (status, timed_out) = wait(&mut child, op.hooks[i].hook.timeout_secs)?;
    log.sync_all().context("syncing identity proof")?;
    let stdout = fs::read_to_string(&probe_path).context("reading identity proof")?;
    writeln!(
        full,
        "\nidentity argv: {argv:?}\nstatus: {status}, timeout={timed_out}\n{stdout}"
    )
    .context("appending identity proof")?;
    full.sync_all().context("syncing identity log")?;
    let expected = identity.expected_stdout.replace("{sha}", &op.landed_sha);
    if !status.success()
        || timed_out
        || stdout.trim() != expected.trim()
        || hash(&target)? != fingerprint
    {
        bail!(
            "installed executable identity did not prove {} at {}",
            op.landed_sha,
            target.display()
        );
    }
    let attempt = op.hooks[i].attempts.last_mut().context("missing attempt")?;
    attempt.installed_sha256 = Some(fingerprint);
    attempt.identity_stdout = Some(stdout.trim().into());
    Ok(())
}

pub(crate) fn restore(op: &PostLandOperation, i: usize) -> Result<()> {
    let Some(attempt) = op.hooks[i].attempts.last() else {
        return Ok(());
    };
    if still_running(attempt)? {
        bail!("cannot restore prior executable while a hook process remains alive");
    }
    let Some(backup) = &attempt.backup else {
        return Ok(());
    };
    let target = binary(op, i)?;
    let temp = target.with_extension(format!("corral-restore-{}", op.key));
    fs::copy(backup, &temp).context("copying prior executable for restoration")?;
    File::open(&temp)?
        .sync_all()
        .context("syncing restored executable")?;
    fs::rename(&temp, &target).context("restoring prior usable executable")?;
    Ok(())
}

//! Bounded argv execution, complete file-backed output, and observable installed identity.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use sha2::{Digest, Sha256};

use crate::hook_process::HookChild;
use crate::hooks::{HookAttempt, HookState, PostLandOperation, save};
use crate::{Ctx, ProcessControl, SystemProcesses};

#[cfg(test)]
#[path = "hook_command_tests.rs"]
mod tests;

fn retry_interrupted<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

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
        let n =
            retry_interrupted(|| file.read(&mut buffer)).context("hashing installed executable")?;
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
        .stderr(log.try_clone().context("cloning hook stderr log")?);
    Ok(cmd)
}

pub(crate) fn still_running(attempt: &HookAttempt) -> Result<bool> {
    if let Some(pid) = attempt.pid
        && let Some(process) = SystemProcesses.inspect(pid)?
        && attempt.process_start.as_deref() == Some(&process.started)
    {
        return Ok(true);
    }
    #[cfg(not(target_os = "linux"))]
    return crate::native_process::has_token(&attempt.token);
    #[cfg(target_os = "linux")]
    {
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
                // Unix copies may be read-only; Windows flushing needs write access.
                OpenOptions::new()
                    .read(true)
                    .write(cfg!(windows))
                    .open(&backup)
                    .context("opening prior binary backup")?
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
    let spawned = HookChild::spawn(command(&hook.argv, attempt, &log)?);
    let child = match spawned {
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
    let (status, timed_out) = child.wait(hook.timeout_secs)?;
    log.sync_all().context("syncing complete hook log")?;
    let attempt = op.hooks[i].attempts.last_mut().context("missing attempt")?;
    attempt.exit_code = status.code();
    #[cfg(unix)]
    {
        attempt.signal = status.signal();
    }
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
    let mut probe = command(&argv, attempt, &log)?;
    probe.stderr(full.try_clone().context("cloning identity stderr log")?);
    let child = HookChild::spawn(probe).context("spawning identity probe")?;
    let (status, timed_out) = child.wait(op.hooks[i].hook.timeout_secs)?;
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
    OpenOptions::new()
        .read(true)
        .write(cfg!(windows))
        .open(&temp)
        .context("opening restored executable")?
        .sync_all()
        .context("syncing restored executable")?;
    fs::rename(&temp, &target).context("restoring prior usable executable")?;
    Ok(())
}

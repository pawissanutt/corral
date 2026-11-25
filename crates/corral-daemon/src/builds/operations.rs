//! Launch, stop and prune managed Builds.
use super::*;
use crate::views::json_len;
use anyhow::anyhow;
use corral_engine::runner::pi::{BuildLaunch, PiRefusal, PiRunner};
use corral_protocol::{
    app::SESSION_PART_MAX,
    build::{BuildRemove, BuildRemoved, BuildStart, CodeStart},
    session::SessionRelaunched,
};
use std::{
    fs::{DirBuilder, File},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::DirBuilderExt,
    process::{Command, Output},
    thread,
};

pub(crate) fn start(shared: &Shared, request: BuildStart) -> Result<AssistantRow> {
    start_host(shared, request, false)
}

pub(crate) fn start_code(shared: &Shared, request: CodeStart) -> Result<AssistantRow> {
    start_host(
        shared,
        BuildStart {
            project: Some(request.project),
            name: request.name,
            here: true,
            profile: None,
            model: request.model,
            effort: request.effort,
            fallback: Vec::new(),
        },
        true,
    )
}

fn launch_host(shared: &Shared, launch: BuildLaunch<'_>, code: bool) -> Result<SessionRef> {
    let placement = shared.chat_placement();
    let runner = PiRunner::new(placement.as_ref());
    let (session, _) = if code {
        runner.start_code(launch)?
    } else {
        runner.start_build(launch)?
    };
    Ok(session)
}

fn start_host(shared: &Shared, request: BuildStart, code: bool) -> Result<AssistantRow> {
    let cwd = if let Some(project) = &request.project {
        shared
            .root()
            .join(shared.engine()?.config.effective_project(project)?.path)
    } else {
        shared.root().to_owned()
    };
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let data_root = shared.build_data_root()?;
    let (id, dir) = reserve(&data_root.join("build"))?;
    let socket_root = shared.private_chat_sockets();
    let launch = BuildLaunch {
        workspace: shared.root(),
        cwd: &cwd,
        data_root: &data_root,
        socket_root: socket_root.as_deref(),
        id: &id,
        resume: false,
        project: request.project.as_deref(),
        name: request.name.as_deref(),
        here: request.here,
        profile: request.profile.as_deref(),
        model: request.model.as_deref(),
        effort: request.effort.as_deref(),
        fallback: &request.fallback,
    };
    let session = match launch_host(shared, launch, code) {
        Ok(started) => started,
        Err(error) => {
            let error = launch_error(&dir, 0, error);
            if let Err(cleanup) = discard_reservation(&dir) {
                return Err(
                    error.context(format!("discard unused Pi build reservation: {cleanup:#}"))
                );
            }
            return Err(error);
        }
    };
    shared.publish();
    let build = require(shared, &id)?;
    require_mode(&build, code)?;
    Ok(build.row(shared, Some(&session)))
}

pub(crate) fn relaunch(shared: &Shared, id: &str) -> Result<SessionRelaunched> {
    restart_host(shared, id, false)
}

pub(crate) fn resume_code(shared: &Shared, id: &str) -> Result<SessionRelaunched> {
    restart_host(shared, id, true)
}

fn require_mode(build: &Build, code: bool) -> Result<()> {
    if !code {
        reject_code(build.record.trusted_code)?;
    } else if !build.record.trusted_code {
        return Err(refuse("expected a Code session", vec![]));
    }
    Ok(())
}

fn restart_host(shared: &Shared, id: &str, code: bool) -> Result<SessionRelaunched> {
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let build = require(shared, id)?;
    require_mode(&build, code)?;
    if build.live(shared)?.is_none() {
        let data_root = shared.build_data_root()?;
        let socket_root = shared.private_chat_sockets();
        let checkout = Path::new(&build.record.repo_root);
        // A missing checkout needs the host's resume diagnosis, not a spawn error.
        let cwd = if checkout.is_dir() {
            checkout
        } else {
            shared.root()
        };
        let launch = BuildLaunch {
            workspace: shared.root(),
            cwd,
            data_root: &data_root,
            socket_root: socket_root.as_deref(),
            id,
            resume: true,
            project: None,
            name: None,
            here: false,
            profile: None,
            model: None,
            effort: None,
            fallback: &[],
        };
        let log = build.dir.join("session.log");
        let start = match fs::metadata(&log) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == ErrorKind::NotFound => 0,
            Err(err) => return Err(err).with_context(|| format!("inspect {}", log.display())),
        };
        launch_host(shared, launch, code).map_err(|err| launch_error(&build.dir, start, err))?;
        shared.publish();
    }
    Ok(SessionRelaunched { id: id.into() })
}

pub(super) fn oversized_status(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PiRefusal>().is_some_and(|refusal| {
        refusal.command == "get_state" && refusal.code == "response_too_large"
    })
}

pub(crate) fn terminate(shared: &Shared, id: &str) -> Result<()> {
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let build = require(shared, id)?;
    reject_code(build.record.trusted_code)?;
    if let Some((session, _)) = build.live(shared)? {
        let SessionRef::Pi { socket, inner, .. } = &session else {
            anyhow::bail!("build placement has no Pi socket");
        };
        // Never signal a terminal's process group. Its host owns terminal teardown.
        let terminal = match probe::<LegacyState>(socket, "get_state", json!({})) {
            Ok(state) => state.mode == Some(SessionMode::Interactive),
            // The old host responded, but may still own a terminal. Don't signal its group.
            Err(error) => oversized_status(&error),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let shutdown = forward::connect(socket, deadline).and_then(|stream| {
            Client::request_on::<_, Value>(stream, "shutdown", json!({}), deadline)
        });
        if terminal {
            shutdown.context("shut down terminal build host")?;
            loop {
                match probe::<Hello>(socket, "hello", json!({})) {
                    Ok(hello) if hello.session_id != id => break,
                    Err(error) if disconnected(&error) => break,
                    _ => {}
                }
                ensure!(Instant::now() < deadline, "Pi build did not shut down");
                thread::sleep(Duration::from_millis(25));
            }
        } else {
            let placement = shared.chat_placement();
            // Socket close precedes the host's exit hook. Give it time to save or
            // clean up the worktree before falling back to the detached runner.
            if shutdown.is_ok() {
                while Instant::now() < deadline {
                    if placement.status(inner)? == SessionStatus::Gone {
                        shared.publish();
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
            if build
                .dir
                .try_exists()
                .context("inspect stopped build directory")?
            {
                placement.stop(inner)?;
            }
        }
    }
    shared.publish();
    Ok(())
}

pub(crate) fn remove(shared: &Shared, request: BuildRemove) -> Result<BuildRemoved> {
    remove_with(shared, request, Command::output)
}

pub(crate) fn remove_with(
    shared: &Shared,
    request: BuildRemove,
    run: impl FnOnce(&mut Command) -> io::Result<Output>,
) -> Result<BuildRemoved> {
    let _lock = shared
        .build_ops
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ensure!(
        is_build(&request.id),
        "invalid: expected build id B-<base36>"
    );
    let data_root = shared.build_data_root()?;
    let build = require_at(shared, &data_root, &request.id)?;
    reject_code(build.record.trusted_code)?;
    if build.live(shared)?.is_some() {
        return Err(refuse("stop the build session before removing it", vec![]));
    }

    let mut command = Command::new("corral-pi");
    command
        .args(["build", "--prune", "--session", &request.id, "--json"])
        .current_dir(shared.root())
        .env("CORRAL_NEXT_ROOT", shared.root());
    if request.dry_run {
        command.arg("--dry-run");
    }
    command.arg("--data-dir").arg(data_root);
    let output = run(&mut command).context("run corral-pi build prune")?;
    if !request.dry_run {
        // Prune can change the record even if it cannot report complete success.
        shared.publish();
    }

    let refusal = |message: String| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            refuse(message, vec![])
        } else {
            let stderr = &stderr[..stderr.floor_char_boundary(2048)];
            refuse(format!("{message}\n{stderr}"), vec![])
        }
    };
    if !output.status.success() {
        return Err(refusal(format!("Pi build prune failed: {}", output.status)));
    }
    let mut removed: BuildRemoved = serde_json::from_slice(&output.stdout)
        .map_err(|error| refusal(format!("invalid Pi build prune result: {error}")))?;
    removed.lines.truncate(16);
    for line in &mut removed.lines {
        line.truncate(line.floor_char_boundary(512));
    }
    ensure!(
        json_len(&removed).context("sizing Pi build prune result")? <= SESSION_PART_MAX,
        forward::PartTooLarge
    );
    Ok(removed)
}

fn reserve(root: &Path) -> Result<(String, PathBuf)> {
    fs::create_dir_all(root).with_context(|| format!("create build store {}", root.display()))?;
    const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let millis = jiff::Timestamp::now().as_millisecond().unsigned_abs();
    let mut time = [b'0'; 13];
    let mut index = time.len();
    let mut n = millis;
    while n > 0 {
        index -= 1;
        time[index] = BASE36[(n % 36) as usize];
        n /= 36;
    }
    let time =
        std::str::from_utf8(&time[index.min(time.len() - 1)..]).context("encode build id")?;
    loop {
        let mut random = [0_u8; 2];
        getrandom::fill(&mut random).map_err(|err| anyhow!("generate build id: {err}"))?;
        let suffix = u16::from_ne_bytes(random) as usize % (36 * 36);
        let id = format!(
            "B-{time}{}{}",
            char::from(BASE36[suffix / 36]),
            char::from(BASE36[suffix % 36])
        );
        let dir = root.join(&id);
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).with_context(|| format!("reserve {}", dir.display())),
        }
    }
}

fn launch_error(dir: &Path, offset: u64, error: anyhow::Error) -> anyhow::Error {
    let lines = (|| -> Result<Vec<String>> {
        let path = dir.join("session.log");
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let len = file.metadata().context("inspect Pi build log")?.len();
        file.seek(SeekFrom::Start(offset.max(len.saturating_sub(8192))))
            .context("seek Pi build log")?;
        let mut text = Vec::new();
        file.take(8192)
            .read_to_end(&mut text)
            .context("read Pi build log")?;
        Ok(String::from_utf8_lossy(&text)
            .lines()
            .rev()
            .filter(|line| !line.trim().is_empty())
            .take(2)
            .map(|line| bounded(line, 512))
            .collect())
    })();
    if let Ok(lines) = &lines
        && let [fix, what] = lines.as_slice()
    {
        return refuse(format!("{what}\n{fix}"), vec![]);
    }
    error
}

fn discard_reservation(dir: &Path) -> Result<()> {
    if dir
        .join("session.json")
        .try_exists()
        .context("inspect reserved build record")?
    {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect reservation {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry.context("read build reservation entry")?;
        if !matches!(
            entry.file_name().to_str(),
            Some("session.log" | "exit_code")
        ) {
            return Ok(());
        }
    }
    // Only retire the daemon's own staging artifacts, never transcripts or host data.
    for file in ["session.log", "exit_code"] {
        let path = dir.join(file);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("remove staged {}", path.display()));
            }
        }
    }
    match fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(err) => {
            Err(err).with_context(|| format!("remove unused reservation {}", dir.display()))
        }
    }
}

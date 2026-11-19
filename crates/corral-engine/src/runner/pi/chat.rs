use std::process::Command;

use super::*;
use crate::config::{Harness, Profile};

pub struct ChatLaunch<'a> {
    pub workspace: &'a Path,
    pub dir: &'a Path,
    pub socket_root: Option<&'a Path>,
    pub harness: &'a Harness,
    pub profile: &'a Profile,
    pub task: Option<&'a str>,
    pub project: Option<&'a str>,
}

impl Client<'_> {
    pub fn hello_if_available(&self) -> Result<Option<Hello>> {
        match self.hello() {
            Ok(hello) => Ok(Some(hello)),
            Err(error) if unavailable(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl PiRunner<'_> {
    /// Chat is always a headless process, even when task sessions run in panes.
    pub fn start_chat(&self, launch: ChatLaunch<'_>) -> Result<(SessionRef, Hello)> {
        let ChatLaunch {
            workspace,
            dir,
            socket_root,
            harness,
            profile,
            task,
            project,
        } = launch;
        private_dir(dir, true)?;
        let dir = dir.canonicalize().context("resolve Pi chat directory")?;
        let socket = chat_socket(socket_root, &dir)?;
        let client = Client::new(&socket);
        match client.hello() {
            Ok(hello) => return Ok((process_ref(dir, socket, hello.pid), hello)),
            Err(error) if unavailable(&error) => {}
            Err(error) => return Err(error),
        }
        let mut command = Command::new("corral-pi");
        command
            .arg("chat")
            .arg("--dir")
            .arg(&dir)
            .arg("--socket")
            .arg(&socket)
            .arg("--headless")
            .arg("--corral")
            .arg(crate::binary_path()?)
            .current_dir(workspace)
            .env("CORRAL_NEXT_ROOT", workspace)
            .env("CORRAL_ACTOR", "assistant")
            .env_remove("CORRAL_TASK")
            .env_remove("CORRAL_RUN")
            .env_remove("CORRAL_FRESH");
        if let Some(task) = task {
            command.env("CORRAL_TASK", task);
        }
        if let Some(project) = project {
            command.args(["--project", project]);
        }
        for (value, flag, name) in [
            (&profile.model, &harness.model_flag, "model_flag"),
            (&profile.effort, &harness.effort_flag, "effort_flag"),
        ] {
            if let Some(value) = value {
                let flag = flag
                    .as_deref()
                    .filter(|flag| !flag.trim().is_empty())
                    .with_context(|| format!("profile requires a harness {name}"))?;
                command.args([flag, value]);
            }
        }
        command.args(&profile.args);
        command.args(profile.fallback_args());
        let inner = self.inner.start_process(command, dir.clone())?;
        let result = self.wait_for_hello(&client, &inner);
        match result {
            Ok(hello) => Ok((
                SessionRef::Pi {
                    socket,
                    inner: Box::new(inner),
                    dir,
                },
                hello,
            )),
            Err(error) => match self.inner.stop(&inner) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("stop failed Pi chat launch: {cleanup:#}")))
                }
            },
        }
    }
}

/// Resolve a detached chat already recorded by its host (including after daemon restart).
pub fn chat_session(dir: &Path, pid: u32, socket_root: Option<&Path>) -> Result<SessionRef> {
    let dir = dir.canonicalize().context("resolve Pi chat directory")?;
    let socket = chat_socket(socket_root, &dir)?;
    Ok(process_ref(dir, socket, pid))
}

fn process_ref(dir: PathBuf, socket: PathBuf, pid: u32) -> SessionRef {
    SessionRef::Pi {
        socket,
        inner: Box::new(SessionRef::Process {
            pid,
            dir: dir.clone(),
        }),
        dir,
    }
}

fn chat_socket(socket_root: Option<&Path>, dir: &Path) -> Result<PathBuf> {
    match socket_root {
        Some(root) => session_socket(root, dir),
        None => session_socket(&runtime_dir(), dir),
    }
}

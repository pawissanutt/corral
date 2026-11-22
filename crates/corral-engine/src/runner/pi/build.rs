use std::{
    borrow::Cow,
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};

use super::{Client, Hello, PiRunner, SessionRef, private_dir, runtime_dir, session_socket};

pub struct BuildLaunch<'a> {
    pub workspace: &'a Path,
    pub cwd: &'a Path,
    pub data_root: &'a Path,
    pub socket_root: Option<&'a Path>,
    pub id: &'a str,
    pub resume: bool,
    pub project: Option<&'a str>,
    pub name: Option<&'a str>,
    pub here: bool,
    pub profile: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub fallback: &'a [String],
}

pub fn is_build_id(id: &str) -> bool {
    id.strip_prefix("B-").is_some_and(|name| {
        (1..=32).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// Match the host's corralPiRoot, including installations still using pi-native.
pub fn pi_data_root() -> Result<PathBuf> {
    let absolute_env = |key| {
        env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let data = match absolute_env("XDG_DATA_HOME") {
        Some(data) => data,
        None => absolute_env("HOME")
            .context("neither XDG_DATA_HOME nor HOME is an absolute path")?
            .join(".local/share"),
    };
    let corral = data.join("corral");
    let pi = corral.join("corral-pi");
    let legacy = corral.join("pi-native");
    Ok(if !pi.exists() && legacy.is_dir() {
        legacy
    } else {
        pi
    })
}

impl PiRunner<'_> {
    pub fn start_build(&self, launch: BuildLaunch<'_>) -> Result<(SessionRef, Hello)> {
        ensure!(
            is_build_id(launch.id),
            "invalid: expected build id B-<base36>"
        );
        let dir = launch.data_root.join("build").join(launch.id);
        private_dir(&dir, true)?;
        let dir = dir.canonicalize().context("resolve Pi build directory")?;
        let socket = derived_socket(launch.socket_root, &dir)?;
        let client = Client::new(&socket);
        ensure!(
            client.hello_if_available()?.is_none(),
            "Pi build socket is already in use"
        );
        let mut command = Command::new("corral-pi");
        command
            .args([
                "build",
                "--headless",
                if launch.resume { "--resume" } else { "--id" },
                launch.id,
            ])
            .arg("--data-dir")
            .arg(launch.data_root)
            .arg("--socket")
            .arg(&socket)
            .arg("--corral")
            .arg(crate::binary_path()?)
            .current_dir(launch.cwd)
            .env("CORRAL_NEXT_ROOT", launch.workspace)
            // A daemon Build's Project default retains its existing route, unlike a new terminal launch.
            .env("CORRAL_PI_MANAGED_BUILD", "1")
            .env_remove("CORRAL_ACTOR")
            .env_remove("CORRAL_TASK")
            .env_remove("CORRAL_RUN")
            .env_remove("CORRAL_FRESH");
        if !launch.resume {
            for (flag, value) in [
                ("--project", launch.project),
                ("--name", launch.name),
                ("--profile", launch.profile),
                ("--model", launch.model),
                ("--effort", launch.effort),
            ] {
                if let Some(value) = value {
                    command.args([flag, value]);
                }
            }
            if launch.here {
                command.arg("--here");
            }
            for model in launch.fallback {
                command.args(["--fallback", model]);
            }
        }
        let inner = self.inner.start_process(command, dir.clone())?;
        let result = self.wait_for_hello(&client, &inner).and_then(|hello| {
            ensure!(
                hello.session_id == launch.id && hello.role == "build",
                "Pi socket answers for a different build session"
            );
            Ok(hello)
        });
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
                    Err(error.context(format!("stop failed Pi build launch: {cleanup:#}")))
                }
            },
        }
    }
}

/// Form a placement only after hello has confirmed the host's identity and PID.
pub fn build_session(dir: &Path, pid: u32, socket: PathBuf) -> SessionRef {
    SessionRef::Pi {
        socket,
        inner: Box::new(SessionRef::Process {
            pid,
            dir: dir.to_owned(),
        }),
        dir: dir.to_owned(),
    }
}

pub fn build_socket<'a>(
    dir: &Path,
    recorded_socket: Option<&'a Path>,
    socket_root: Option<&Path>,
) -> Result<Cow<'a, Path>> {
    if let Some(socket) = recorded_socket {
        ensure!(
            socket.is_absolute(),
            "recorded Pi build socket is not absolute"
        );
        return Ok(Cow::Borrowed(socket));
    }
    let dir = dir
        .canonicalize()
        .context("resolve recorded Pi build directory")?;
    Ok(Cow::Owned(derived_socket(socket_root, &dir)?))
}

fn derived_socket(root: Option<&Path>, dir: &Path) -> Result<PathBuf> {
    match root {
        Some(root) => session_socket(root, dir),
        None => session_socket(&runtime_dir(), dir),
    }
}

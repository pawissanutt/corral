//! Service definitions are written only on an explicit owner command; never enabled here.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::Store;

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is required to install a user service")
}

fn service_path(store: &Store) -> Result<PathBuf> {
    let id = corral_core::agent::paths::store_id(store);
    if cfg!(target_os = "linux") {
        Ok(home()?.join(format!(".config/systemd/user/corrald@{id}.service")))
    } else if cfg!(target_os = "macos") {
        Ok(home()?.join(format!(
            "Library/LaunchAgents/dev.corral.corrald.{id}.plist"
        )))
    } else {
        bail!("no supported user service manager; use `corral daemon run`")
    }
}

pub(crate) fn installed(store: &Store) -> Result<bool> {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return Ok(false);
    }
    Ok(service_path(store)?.is_file())
}

fn clean_path(path: &OsStr) -> Result<String> {
    let mut dirs = Vec::new();
    for dir in std::env::split_paths(path) {
        if dir.is_absolute() && dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs)
        .context("joining service PATH")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("service PATH is not UTF-8"))
}

fn unit_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn definition(
    exe: &Path,
    root: &Path,
    id: &str,
    socket: Option<&Path>,
    path: &str,
    macos: bool,
) -> Result<String> {
    let exe = exe.to_str().context("executable path is not UTF-8")?;
    let root = root.to_str().context("workspace root is not UTF-8")?;
    let socket = socket
        .map(|socket| socket.to_str().context("herdr socket path is not UTF-8"))
        .transpose()?;
    if macos {
        let mut environment = format!("<key>PATH</key><string>{}</string>", xml(path));
        if let Some(socket) = socket {
            environment.push_str(&format!(
                "<key>HERDR_SOCKET_PATH</key><string>{}</string>",
                xml(socket)
            ));
        }
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>dev.corral.corrald.{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>--root</string><string>{}</string><string>daemon</string><string>run</string></array>\n<key>EnvironmentVariables</key><dict>{environment}</dict>\n<key>KeepAlive</key><true/>\n<key>RunAtLoad</key><true/>\n</dict></plist>\n",
            xml(id),
            xml(exe),
            xml(root)
        ))
    } else {
        let mut environment = format!("Environment={}\n", unit_quote(&format!("PATH={path}")));
        if let Some(socket) = socket {
            environment.push_str(&format!(
                "Environment={}\n",
                unit_quote(&format!("HERDR_SOCKET_PATH={socket}"))
            ));
        }
        // ExecStart is not a shell: quote arguments for systemd, and escape its $ expansion.
        Ok(format!(
            "[Unit]\nDescription=Corral workspace daemon\n\n[Service]\nExecStart={} --root {} daemon run\n{environment}Restart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
            unit_quote(exe).replace('$', "$$"),
            unit_quote(root).replace('$', "$$")
        ))
    }
}

pub(crate) fn install(store: &Store, print: bool, force: bool) -> Result<()> {
    let target = service_path(store)?;
    let exe = std::env::current_exe()
        .context("finding corral executable")?
        .canonicalize()
        .context("resolving corral executable")?;
    let root = store
        .root()
        .canonicalize()
        .context("resolving workspace root")?;
    let binding = store.herdr_session_binding()?;
    let path = clean_path(&std::env::var_os("PATH").unwrap_or_default())?;
    let id = corral_core::agent::paths::store_id(store);
    let contents = definition(
        &exe,
        &root,
        &id,
        binding.as_ref().map(|binding| binding.socket.as_path()),
        &path,
        cfg!(target_os = "macos"),
    )?;
    if print {
        print!("{contents}");
        return Ok(());
    }
    match std::fs::read_to_string(&target) {
        Ok(existing) if existing != contents && !force => bail!(
            "{} already contains a different service; use --force to replace it",
            target.display()
        ),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", target.display())),
    }
    std::fs::create_dir_all(target.parent().context("service path has no parent")?)
        .context("creating user service directory")?;
    std::fs::write(&target, contents).with_context(|| format!("writing {}", target.display()))?;
    println!("Wrote {}", target.display());
    if cfg!(target_os = "linux") {
        println!(
            "systemctl --user daemon-reload\nsystemctl --user enable --now corrald@{id}.service\nFor start at boot, ask the administrator to run `loginctl enable-linger <user>`."
        );
    } else {
        println!(
            "launchctl bootstrap gui/$(id -u) {}",
            shell_quote(&target.to_string_lossy())
        );
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_quotes_space_root_and_filters_relative_missing_duplicate_path_entries() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let path = std::env::join_paths([
            Path::new("relative"),
            bin.as_path(),
            &dir.path().join("missing"),
            bin.as_path(),
        ])
        .unwrap();
        let cleaned = clean_path(&path).unwrap();
        assert_eq!(cleaned, bin.to_str().unwrap());
        let root = dir.path().join("root with space");
        let unit = definition(
            Path::new("/opt/corral"),
            &root,
            "st-test",
            Some(Path::new("/tmp/herdr.sock")),
            &cleaned,
            false,
        )
        .unwrap();
        assert!(unit.contains(&format!(
            "--root {} daemon run",
            unit_quote(root.to_str().unwrap())
        )));
        assert!(unit.contains(&format!(
            "Environment={}",
            unit_quote(&format!("PATH={cleaned}"))
        )));
        assert!(unit.contains("HERDR_SOCKET_PATH=/tmp/herdr.sock"));
    }
}

//! Private output validation. Walks are capped; deletion uses the existing no-follow deleter.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use corral_core::TaskHead;
use corral_core::fs_guard::DirHandle;
use corral_core::output::{OutputCfg, OutputKind, protected_component, protected_extension};

use super::Owner;
use crate::Ctx;

pub(super) const WALK_LIMIT: usize = 20_000;

fn git_status(root: &Path, args: &[&str]) -> Result<i32> {
    corral_git::work::record();
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let status = command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("checking Git output ownership")?;
    status.code().context("Git ownership inspection terminated")
}

pub(super) fn location(
    store: &corral_core::Store,
    task: &str,
    project: &str,
    worktree: &Path,
    cfg: &OutputCfg,
) -> Result<PathBuf> {
    cfg.validate()?;
    let repo = store
        .project_path(project)?
        .canonicalize()
        .context("resolve output project")?;
    ensure!(
        worktree == repo.join(".worktrees").join(task),
        "shared or external worktree"
    );
    DirHandle::open(worktree)?;
    ensure!(
        corral_git::git_common_dir(worktree)? == corral_git::git_common_dir(&repo)?,
        "unregistered worktree"
    );
    ensure!(
        std::fs::symlink_metadata(worktree.join(".git"))?.is_file(),
        "not a private Git worktree"
    );
    let pathspec = format!(":(literal){}", cfg.path);
    ensure!(
        git_status(worktree, &["ls-files", "--error-unmatch", "--", &pathspec])? == 1,
        "output contains tracked files or Git ownership is unreadable"
    );
    ensure!(
        git_status(
            worktree,
            &[
                "check-ignore",
                "-q",
                "--no-index",
                "--",
                &format!("{}/", cfg.path)
            ]
        )? == 0,
        "output is not Git-ignored"
    );
    Ok(worktree.join(&cfg.path))
}

pub(super) fn identity(
    ctx: &Ctx,
    head: &TaskHead,
    cfg: &OutputCfg,
    owner: &Owner,
) -> Result<PathBuf> {
    let path = location(
        &ctx.store,
        &head.id,
        head.project.as_deref().context("unknown project")?,
        &head
            .current_run
            .as_deref()
            .context("no producing run")?
            .worktree,
        cfg,
    )?;
    ensure!(
        owner.version == 1
            && owner.project == head.project.as_deref().unwrap_or("")
            && owner.task == head.id
            && owner.run == head.current_run.as_deref().context("run disappeared")?.id
            && owner.config.ownership_eq(cfg)
            && owner.path == path,
        "output ownership/generation/configuration changed"
    );
    let worktree = DirHandle::open(&owner.worktree)?;
    ensure!(
        worktree.identity().dev == owner.root_dev && worktree.identity().ino == owner.root_ino,
        "worktree identity changed"
    );
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let output = DirHandle::open(&path)?;
            private_root(&path)?;
            ensure!(
                output.identity().dev == owner.dev && output.identity().ino == owner.ino,
                "output identity changed (not registered)"
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect registered output"),
    }
    Ok(path)
}

/// Refuse Cargo routing overrides instead of guessing the effective target/build directory.
/// Only the default worktree-local target contract is currently supported.
pub(super) fn cargo_routing(worktree: &Path) -> Result<()> {
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        ensure!(
            !(key.starts_with("CARGO_")
                && (key.contains("TARGET_DIR") || key.contains("BUILD_DIR"))),
            "Cargo output environment override: {key}"
        );
    }
    let mut configs = Vec::new();
    for ancestor in worktree.ancestors() {
        configs.push(ancestor.join(".cargo/config"));
        configs.push(ancestor.join(".cargo/config.toml"));
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .context("unknown Cargo configuration home")?;
    ensure!(
        cargo_home.is_absolute(),
        "relative Cargo configuration home cannot be resolved safely"
    );
    configs.push(cargo_home.join("config"));
    configs.push(cargo_home.join("config.toml"));
    for config in configs {
        let mut file = match corral_core::fs_guard::open_regular_nofollow(&config) {
            Ok(file) => file,
            Err(error)
                if error.chain().any(|e| {
                    e.downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                }) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("unreadable Cargo configuration"),
        };
        let mut bytes = Vec::new();
        file.by_ref().take(65_537).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 65_536,
            "Cargo configuration exceeds inspection bound"
        );
        let text = std::str::from_utf8(&bytes).context("non-UTF-8 Cargo configuration")?;
        corral_core::output::require_default_cargo_output(text)
            .with_context(|| format!("Cargo output configuration in {}", config.display()))?;
    }
    Ok(())
}

/// Cargo's build-script OUT_DIR has its own explicit producer marker. This is not a
/// blanket exception for source files anywhere under target.
fn cargo_generated_source(root: &Path, file: &Path, extension: &str) -> Result<bool> {
    if !matches!(extension, "rs" | "c" | "h" | "cpp" | "hpp") {
        return Ok(false);
    }
    let relative = file.strip_prefix(root)?;
    let parts: Vec<_> = relative.components().collect();
    let Some(build) = parts.iter().position(|p| p.as_os_str() == "build") else {
        return Ok(false);
    };
    if !(1..=2).contains(&build)
        || parts.get(build + 2).is_none_or(|p| p.as_os_str() != "out")
        || parts.len() <= build + 3
    {
        return Ok(false);
    }
    let package = root.join(parts[..build + 2].iter().collect::<PathBuf>());
    let out = package.join("out");
    let mut bytes = Vec::new();
    corral_core::fs_guard::open_regular_nofollow(&package.join("root-output"))?
        .take(4097)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4096,
        "Cargo generated-source marker exceeds bound"
    );
    let declared = std::str::from_utf8(&bytes)?.trim_end_matches('\n');
    ensure!(
        Path::new(declared) == out,
        "Cargo OUT_DIR marker does not own generated source"
    );
    DirHandle::open(&out)?;
    Ok(true)
}

pub(super) fn private_root(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path)?;
        ensure!(
            meta.uid() == rustix::process::getuid().as_raw() && meta.mode() & 0o077 == 0,
            "output has shared permissions or unknown owner"
        );
    }
    ensure!(
        cfg!(unix),
        "private output ownership unsupported on this platform"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn no_mounts_below(path: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    std::fs::File::open("/proc/self/mountinfo")?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1_048_576, "mount visibility bound exceeded");
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let field = line
            .split(|b| *b == b' ')
            .nth(4)
            .context("incomplete mount visibility")?;
        let mut decoded = Vec::new();
        let mut at = 0;
        while at < field.len() {
            if field[at] == b'\\' {
                let octal = field
                    .get(at + 1..at + 4)
                    .context("unknown mount path escape")?;
                ensure!(
                    octal.iter().all(|b| (b'0'..=b'7').contains(b)),
                    "unknown mount path escape"
                );
                let value = (u16::from(octal[0] - b'0') * 64)
                    + (u16::from(octal[1] - b'0') * 8)
                    + u16::from(octal[2] - b'0');
                decoded.push(u8::try_from(value).context("invalid mount path escape")?);
                at += 4;
            } else {
                decoded.push(field[at]);
                at += 1;
            }
        }
        use std::os::unix::ffi::OsStringExt;
        let mount = PathBuf::from(std::ffi::OsString::from_vec(decoded));
        ensure!(
            !mount.starts_with(path),
            "mounted/shared output subtree: {}",
            mount.display()
        );
    }
    Ok(())
}

pub(super) fn contents(ctx: &Ctx, cfg: &OutputCfg, worktree: &Path, path: &Path) -> Result<u64> {
    if !path.try_exists()? {
        return Ok(0);
    }
    let root = DirHandle::open(path)?;
    #[cfg(target_os = "linux")]
    no_mounts_below(path)?;
    let mut pending = vec![path.to_path_buf()];
    let (mut entries, mut bytes) = (0usize, 0u64);
    while let Some(dir) = pending.pop() {
        let handle = DirHandle::open(&dir)?;
        ensure!(
            handle.identity().dev == root.identity().dev,
            "shared/mounted output subtree"
        );
        for entry in std::fs::read_dir(&dir).context("list output directory")? {
            entries += 1;
            ensure!(
                entries <= WALK_LIMIT,
                "output inspection entry bound exceeded"
            );
            let entry = entry.context("read output entry")?;
            let path = entry.path();
            let name = entry
                .file_name()
                .to_str()
                .context("non-UTF-8 output name")?
                .to_owned();
            ensure!(
                !protected_component(&name),
                "protected source/evidence/cache component: {name}"
            );
            corral_core::work::counted(corral_core::work::Counter::FileStats, 1);
            let meta = std::fs::symlink_metadata(&path).context("inspect output entry")?;
            ensure!(
                !meta.file_type().is_symlink(),
                "symlink in output: {}",
                path.display()
            );
            ensure!(meta.is_dir() || meta.is_file(), "special file in output");
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(meta.dev() == root.identity().dev, "mounted/shared output");
                ensure!(
                    meta.uid() == rustix::process::getuid().as_raw(),
                    "unknown output file owner"
                );
                ensure!(
                    meta.is_dir() || meta.nlink() == 1,
                    "shared hardlinked output file"
                );
            }
            let relative = path
                .strip_prefix(worktree)
                .context("output escaped worktree")?
                .to_string_lossy();
            ensure!(
                !ctx.store
                    .config
                    .artifacts
                    .globs
                    .iter()
                    .any(|glob| crate::artifacts::glob_match(glob, &relative)),
                "output contains evidence matching artifact policy"
            );
            if meta.is_dir() {
                pending.push(path);
                continue;
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let generated_source = cfg.kind == OutputKind::CargoTarget
                && cargo_generated_source(root.path(), &path, &extension)?;
            ensure!(
                !protected_extension(&extension) || generated_source,
                "source/evidence file in output: {name}"
            );
            match cfg.kind {
                OutputKind::Generated => ensure!(
                    cfg.extensions.contains(&extension),
                    "undeclared generated file type: {name}"
                ),
                OutputKind::CargoTarget => {
                    ensure!(
                        generated_source
                            || extension.is_empty()
                            || matches!(
                                extension.as_str(),
                                "json"
                                    | "d"
                                    | "rlib"
                                    | "rmeta"
                                    | "so"
                                    | "a"
                                    | "o"
                                    | "bc"
                                    | "dylib"
                                    | "dll"
                                    | "exe"
                                    | "pdb"
                                    | "bin"
                                    | "stamp"
                                    | "lock"
                                    | "html"
                                    | "css"
                                    | "js"
                                    | "svg"
                                    | "png"
                                    | "woff2"
                                    | "ico"
                                    | "ttf"
                                    | "map"
                                    | "wasm"
                                    | "timestamp"
                                    | "tag"
                            ),
                        "unknown Cargo output file type: {name}"
                    );
                    if name == ".cargo-lock" {
                        let file = corral_core::fs_guard::open_regular_nofollow(&path)?;
                        file.try_lock()
                            .map_err(|e| anyhow::anyhow!("active Cargo build: {e}"))?;
                    }
                }
            }
            bytes = bytes
                .checked_add(meta.len())
                .context("output size overflow")?;
        }
        handle.revalidate()?;
    }
    root.revalidate()?;
    if cfg.kind == OutputKind::CargoTarget && bytes > 0 {
        let mut signature = [0u8; 43];
        corral_core::fs_guard::open_regular_nofollow(&path.join("CACHEDIR.TAG"))?
            .read_exact(&mut signature)?;
        ensure!(
            &signature == b"Signature: 8a477f597d28d172789f06886806bc55",
            "invalid Cargo cache tag"
        );
    }
    Ok(bytes)
}

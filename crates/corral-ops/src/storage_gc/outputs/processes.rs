//! Strict Linux visibility. Unlike scratch's advisory probe, an unreadable resident defers.
use std::io::Read;
use std::path::Path;
#[cfg(all(test, unix))]
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};

const MAX_PROCESSES: usize = 8192;
const MAX_LINKS: usize = 65_536;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILE: u64 = 1024 * 1024;

#[derive(Default)]
struct Budget {
    processes: usize,
    links: usize,
    bytes: usize,
}

fn read(path: &Path, budget: &mut Budget) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("incomplete process visibility: {}", path.display()))?
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .context("read process visibility")?;
    budget.bytes += bytes.len();
    ensure!(
        bytes.len() as u64 <= MAX_FILE && budget.bytes <= MAX_BYTES,
        "process visibility byte bound exceeded"
    );
    Ok(bytes)
}

fn contains_path(bytes: &[u8], path: &Path) -> bool {
    let needle = path.as_os_str().as_encoded_bytes();
    !needle.is_empty()
        && bytes.windows(needle.len()).enumerate().any(|(at, part)| {
            part == needle
                && bytes
                    .get(at + needle.len())
                    .is_none_or(|b| matches!(b, b'/' | b'\0' | b' ' | b'\n' | b'"' | b'\''))
        })
}

fn uses(bytes: &[u8], task: &str, worktree: &Path) -> bool {
    contains_path(bytes, worktree)
        || bytes
            .split(|b| *b == 0)
            .any(|entry| entry.strip_prefix(b"CORRAL_SANDBOX_TASK=") == Some(task.as_bytes()))
}

fn started(stat: &[u8]) -> Result<&[u8]> {
    let end = stat
        .iter()
        .rposition(|b| *b == b')')
        .context("unknown process identity")?;
    stat[end + 1..]
        .split(|b| b.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .nth(19)
        .context("incomplete process identity")
}

fn link(path: &Path, worktree: &Path, budget: &mut Budget) -> Result<bool> {
    budget.links += 1;
    ensure!(
        budget.links <= MAX_LINKS,
        "process descriptor bound exceeded"
    );
    let target = std::fs::read_link(path)
        .with_context(|| format!("incomplete process visibility: {}", path.display()))?;
    Ok(target.starts_with(worktree)
        || contains_path(target.as_os_str().as_encoded_bytes(), worktree))
}

pub(super) fn unused(
    task: &str,
    worktree: &Path,
    owner: &dyn Fn(&[u8]) -> Result<bool>,
) -> Result<()> {
    corral_core::work::counted(corral_core::work::Counter::Processes, 1);
    ensure!(
        cfg!(target_os = "linux"),
        "complete consumer visibility unsupported on this platform"
    );
    ensure!(
        worktree
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .all(|b| b.is_ascii() && !b.is_ascii_whitespace() && *b != b'\\'),
        "process path escaping cannot be inspected completely"
    );
    // Linux reserves this inode for the initial PID namespace. An unrecognized kernel
    // or nested namespace is not proof that this /proc can see every consumer.
    ensure!(
        std::fs::read_link("/proc/self/ns/pid")? == Path::new("pid:[4026531836]"),
        "complete process visibility requires the recognized initial Linux PID namespace"
    );
    let mut budget = Budget::default();
    let mounts = read(Path::new("/proc/self/mountinfo"), &mut budget)?;
    ensure!(
        !mounts
            .split(|b| matches!(b, b',' | b' ' | b'\n'))
            .any(|part| part.starts_with(b"hidepid=")
                && part != b"hidepid=0"
                && part != b"hidepid=off"),
        "incomplete process visibility: hidepid mount"
    );
    inspect_owners(
        Path::new("/proc"),
        task,
        worktree,
        Some(std::process::id()),
        owner,
    )
}

#[cfg(all(test, unix))]
fn inspect(proc: &Path, task: &str, worktree: &Path, own: Option<u32>) -> Result<()> {
    inspect_owners(proc, task, worktree, own, &|_| {
        anyhow::bail!("unknown process owner")
    })
}

fn inspect_owners(
    proc: &Path,
    task: &str,
    worktree: &Path,
    own: Option<u32>,
    owner: &dyn Fn(&[u8]) -> Result<bool>,
) -> Result<()> {
    let mut budget = Budget::default();
    for entry in std::fs::read_dir(proc).context("incomplete process inventory")? {
        let entry = entry.context("incomplete process inventory entry")?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        budget.processes += 1;
        ensure!(
            budget.processes <= MAX_PROCESSES,
            "process inventory bound exceeded"
        );
        let base = entry.path();
        let result = (|| -> Result<()> {
            let before = read(&base.join("stat"), &mut budget)?;
            let status = read(&base.join("status"), &mut budget)?;
            if status.split(|b| *b == b'\n').any(|line| {
                line.starts_with(b"State:\tZ")
                    || line.starts_with(b"State:\tX")
                    || line == b"Kthread:\t1"
            }) {
                return Ok(());
            }
            // The collector itself is not a build, but executing this very output is still use.
            ensure!(
                !link(&base.join("exe"), worktree, &mut budget)?,
                "active generated-executable consumer {pid}"
            );
            if Some(pid) == own {
                ensure!(
                    !contains_path(&read(&base.join("maps"), &mut budget)?, worktree),
                    "collector maps generated output"
                );
                for fd in std::fs::read_dir(base.join("fd"))
                    .context("incomplete collector descriptor visibility")?
                {
                    let fd = fd.context("incomplete collector descriptor entry")?;
                    if fd.path().is_dir() {
                        continue;
                    } // Only the deleter's retained directory handles.
                    match link(&fd.path(), worktree, &mut budget) {
                        Ok(false) => {}
                        Ok(true) => anyhow::bail!("collector holds an output file descriptor"),
                        Err(error)
                            if !fd.path().exists()
                                && error.chain().any(|e| {
                                    e.downcast_ref::<std::io::Error>()
                                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                                }) => {}
                        Err(error) => return Err(error),
                    }
                }
                return Ok(());
            }
            ensure!(
                !link(&base.join("cwd"), worktree, &mut budget)?,
                "active worker/build/LSP consumer {pid}"
            );
            let root = std::fs::read_link(base.join("root"))
                .context("incomplete process namespace visibility")?;
            ensure!(
                root == Path::new("/"),
                "unknown process namespace for {pid}"
            );
            for field in ["cmdline", "environ", "maps"] {
                let bytes = read(&base.join(field), &mut budget)?;
                if field == "environ" {
                    for token in bytes
                        .split(|b| *b == 0)
                        .filter_map(|entry| entry.strip_prefix(b"CORRAL_PROCESS_OWNER="))
                    {
                        ensure!(
                            !owner(token)?,
                            "active owned worker/gate/reviewer descendant {pid}"
                        );
                    }
                }
                ensure!(
                    !uses(&bytes, task, worktree),
                    "active or unknown-owner {field} consumer {pid}"
                );
            }
            for fd in std::fs::read_dir(base.join("fd"))
                .context("incomplete process descriptor visibility")?
            {
                let fd = fd.context("incomplete process descriptor entry")?;
                match link(&fd.path(), worktree, &mut budget) {
                    Ok(false) => {}
                    Ok(true) => anyhow::bail!("active output descriptor consumer {pid}"),
                    Err(error)
                        if !fd.path().exists()
                            && error.chain().any(|e| {
                                e.downcast_ref::<std::io::Error>()
                                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                            }) => {}
                    Err(error) => return Err(error),
                }
            }
            let after = read(&base.join("stat"), &mut budget)?;
            ensure!(
                started(&before)? == started(&after)?,
                "process identity changed during observation"
            );
            Ok(())
        })();
        if let Err(error) = result {
            // Exit during the scan is safe; permission failures and still-live partial reads aren't.
            if std::fs::symlink_metadata(&base)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn process(root: &Path) -> PathBuf {
        use std::os::unix::fs::symlink;
        let base = root.join("12");
        std::fs::create_dir_all(base.join("fd")).unwrap();
        std::fs::write(
            base.join("stat"),
            format!("12 (fixture) S {}", ["1"; 25].join(" ")),
        )
        .unwrap();
        std::fs::write(base.join("status"), "State:\tS\n").unwrap();
        for name in ["cmdline", "environ", "maps"] {
            std::fs::write(base.join(name), b"").unwrap();
        }
        for name in ["cwd", "root"] {
            symlink("/", base.join(name)).unwrap();
        }
        symlink("/bin/fixture", base.join("exe")).unwrap();
        base
    }

    #[test]
    #[cfg(unix)]
    fn visibility_and_all_consumer_channels_fail_closed() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let base = process(tmp.path());
        let worktree = Path::new("/private/task");
        assert!(inspect(tmp.path(), "T-001", worktree, None).is_ok());
        for (field, value) in [
            ("cmdline", b"rust-analyzer\0/private/task\0".as_slice()),
            ("environ", b"CORRAL_SANDBOX_TASK=T-001\0"),
            ("maps", b"0000 r-xp /private/task/target/program\n"),
        ] {
            std::fs::write(base.join(field), value).unwrap();
            assert!(
                inspect(tmp.path(), "T-001", worktree, None).is_err(),
                "{field}"
            );
            std::fs::write(base.join(field), b"").unwrap();
        }
        for field in ["exe", "cwd"] {
            let previous = std::fs::read_link(base.join(field)).unwrap();
            std::fs::remove_file(base.join(field)).unwrap();
            symlink(worktree.join("target/program"), base.join(field)).unwrap();
            assert!(
                inspect(tmp.path(), "T-001", worktree, None).is_err(),
                "{field}"
            );
            std::fs::remove_file(base.join(field)).unwrap();
            symlink(previous, base.join(field)).unwrap();
        }
        symlink(worktree.join("dist/app.js"), base.join("fd/4")).unwrap();
        assert!(inspect(tmp.path(), "T-001", worktree, None).is_err());
        std::fs::remove_file(base.join("fd/4")).unwrap();
        std::fs::write(base.join("environ"), b"CORRAL_PROCESS_OWNER=fixture\0").unwrap();
        assert!(inspect(tmp.path(), "T-001", worktree, None).is_err());
        assert!(inspect_owners(tmp.path(), "T-001", worktree, None, &|_| Ok(true)).is_err());
        assert!(inspect_owners(tmp.path(), "T-001", worktree, None, &|_| Ok(false)).is_ok());
        std::fs::write(base.join("environ"), b"").unwrap();
        let own_output = tmp.path().join("own-output");
        std::fs::create_dir(&own_output).unwrap();
        std::fs::write(own_output.join("app.bin"), b"x").unwrap();
        symlink(own_output.join("app.bin"), base.join("fd/5")).unwrap();
        assert!(inspect(tmp.path(), "T-001", &own_output, Some(12)).is_err());
        std::fs::remove_file(base.join("fd/5")).unwrap();
        std::fs::remove_file(base.join("maps")).unwrap();
        assert!(inspect(tmp.path(), "T-001", worktree, None).is_err());
    }
}

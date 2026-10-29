//! Asset checks shared by dispatch and the launcher: the asset digest, the GO manifest's Bun
//! range, the Bun on PATH and the fixed-path role, delivery and auth modules.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::agent::{AgentRole, paths};
use serde_json::Value;

use crate::agent::launch::LaunchRefusal;

const ASSET_FILES_MAX: usize = 4096;
/// Room for the compiled host (about 100 MiB: Bun plus every bundled dependency).
const ASSET_BYTES_MAX: u64 = 512 * 1024 * 1024;
/// The release host `bun build --compile` makes from `src/host/compiled.ts`: it embeds Bun and
/// its role, delivery and auth modules, so its installation has no `src/` modules or Bun to find.
pub const HOST_BINARY: &str = "corral-pi-host";

/// The compiled host of a release installation, when `assets` is one.
pub fn compiled_host(assets: &Path) -> Option<PathBuf> {
    let path = assets.join(HOST_BINARY);
    std::fs::symlink_metadata(&path)
        .is_ok_and(|meta| meta.is_file())
        .then_some(path)
}

/// Whether `assets` supplies `module`: as a source file, or bundled inside a compiled host.
pub fn has_module(assets: &Path, module: &str) -> bool {
    compiled_host(assets).is_some() || assets.join(module).is_file()
}

fn refuse(code: &'static str, detail: impl Into<String>) -> LaunchRefusal {
    LaunchRefusal {
        code,
        detail: detail.into(),
    }
}

pub struct CompatPins {
    pub range: String,
    /// Catalog keys the installed host implements; a manifest without the list implements none.
    pub capabilities: BTreeSet<String>,
}

pub fn read_compat(assets: &Path) -> Result<CompatPins, LaunchRefusal> {
    let path = assets.join("compat.json");
    let value: Option<Value> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let range = value
        .as_ref()
        .filter(|v| v["verdict"] == "GO")
        .and_then(|v| v["bun"]["range"].as_str().map(str::to_owned));
    let capabilities = value
        .as_ref()
        .and_then(|v| v["capabilities"].as_array())
        .map(|keys| {
            keys.iter()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    range
        .map(|range| CompatPins {
            range,
            capabilities,
        })
        .ok_or_else(|| {
            refuse(
                "compat_unavailable",
                format!(
                    "no GO compatibility manifest with a Bun range at {}",
                    path.display()
                ),
            )
        })
}

/// `(code, detail)` for each missing module, role first, each naming its owning child.
pub fn missing_modules(assets: &Path, role: AgentRole) -> Vec<(&'static str, String)> {
    let role_name = role.as_str();
    let modules = [
        (
            format!("src/roles/{role_name}/index.ts"),
            "role_unavailable",
            format!(
                "{role_name} (owner {})",
                crate::agent::hooks::preflight::role_owner(role)
            ),
        ),
        (
            "src/delivery/index.ts".to_owned(),
            "delivery_unavailable",
            format!("(owner {})", crate::agent::delivery::OWNER),
        ),
        (
            "src/auth/index.ts".to_owned(),
            "credentials_unavailable",
            format!("(owner {})", crate::agent::credentials::OWNER),
        ),
    ];
    modules
        .into_iter()
        .filter(|(module, _, _)| !has_module(assets, module))
        .map(|(_, code, detail)| (code, detail))
        .collect()
}

/// SHA-256 over every asset file a managed installation ships (`package.json`, `bun.lock`,
/// `compat.json`, `fixture.json` when present, `src/**` and `assets/**`): one
/// `<sha256>  <relative path>\n` line per file, in byte order. `node_modules/` and `test/` are
/// outside it; a symlink inside it is refused.
pub fn asset_digest(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    for name in [
        "package.json",
        "bun.lock",
        "compat.json",
        "fixture.json",
        HOST_BINARY,
    ] {
        let path = dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => files.push(PathBuf::from(name)),
            Ok(_) => bail!("{} is not a regular file", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
        }
    }
    for tree in ["src", "assets"] {
        if dir.join(tree).exists() {
            collect(dir, Path::new(tree), &mut files)?;
        }
    }
    files.sort();
    let mut manifest = String::new();
    let mut total = 0u64;
    for file in &files {
        let bytes = std::fs::read(dir.join(file))
            .with_context(|| format!("reading {}", dir.join(file).display()))?;
        total += bytes.len() as u64;
        if files.len() > ASSET_FILES_MAX || total > ASSET_BYTES_MAX {
            bail!(
                "{} exceeds {ASSET_FILES_MAX} files or {ASSET_BYTES_MAX} bytes",
                dir.display()
            );
        }
        let relative = file.to_str().context("asset paths are UTF-8")?;
        manifest.push_str(&format!("{}  {relative}\n", paths::sha256_hex(&bytes)));
    }
    Ok(paths::sha256_hex(manifest.as_bytes()))
}

fn collect(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let path = root.join(relative);
    let meta = std::fs::symlink_metadata(&path)
        .with_context(|| format!("inspecting {}", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!("{} is a symlink inside the asset tree", path.display());
    }
    if meta.is_file() {
        out.push(relative.to_path_buf());
        return Ok(());
    }
    for entry in std::fs::read_dir(&path).with_context(|| format!("listing {}", path.display()))? {
        collect(root, &relative.join(entry?.file_name()), out)?;
        if out.len() > ASSET_FILES_MAX {
            bail!("{} holds more than {ASSET_FILES_MAX} files", root.display());
        }
    }
    Ok(())
}

pub fn find_bun() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("bun"))
        .find(|candidate| executable(candidate))
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

pub fn bun_version(bun: &Path) -> Result<String, LaunchRefusal> {
    let output = std::process::Command::new(bun)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|error| refuse("bun_unavailable", format!("{}: {error}", bun.display())))?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || version.is_empty() {
        return Err(refuse(
            "bun_unavailable",
            format!("{} --version failed", bun.display()),
        ));
    }
    Ok(version)
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// A semver range of `||` alternatives, each space-separated comparators `[=|>=|<=|>|<]x.y.z`.
pub fn version_satisfies(version: &str, range: &str) -> bool {
    let Some(have) = parse_version(version) else {
        return false;
    };
    range.split("||").any(|alternative| {
        let comparators: Vec<&str> = alternative.split_whitespace().collect();
        !comparators.is_empty()
            && comparators.iter().all(|comparator| {
                let (op, want) = [">=", "<=", ">", "<", "="]
                    .iter()
                    .find_map(|op| comparator.strip_prefix(op).map(|rest| (*op, rest)))
                    .unwrap_or(("=", comparator));
                parse_version(want).is_some_and(|want| match op {
                    ">=" => have >= want,
                    "<=" => have <= want,
                    ">" => have > want,
                    "<" => have < want,
                    _ => have == want,
                })
            })
    })
}

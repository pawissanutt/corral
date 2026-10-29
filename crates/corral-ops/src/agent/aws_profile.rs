//! Read-only named AWS profile inspection and the non-secret Bedrock login binding.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::credentials::{auth_dir, ensure_auth_dir};

const BINDING_FILE: &str = "amazon-bedrock.json";
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsBinding {
    pub profile: String,
    pub region: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredBinding {
    #[serde(rename = "type")]
    kind: String,
    profile: String,
    region: String,
}

fn check_profile_name(profile: &str) -> Result<()> {
    if profile.is_empty()
        || profile.len() > 64
        || !profile
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.+=,@-".contains(&c))
    {
        bail!("credentials_unavailable: invalid AWS profile name");
    }
    Ok(())
}

pub fn check_region(region: &str) -> Result<()> {
    let parts: Vec<&str> = region.split('-').collect();
    let valid = parts.len() >= 3
        && parts[0].len() == 2
        && parts[0].bytes().all(|c| c.is_ascii_lowercase())
        && parts[1..parts.len() - 1]
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_lowercase()))
        && parts
            .last()
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()));
    if !valid
        || region.starts_with("cn-")
        || region.starts_with("us-gov-")
        || region.starts_with("us-iso")
        || region.starts_with("eu-isoe-")
    {
        bail!("network_refused: AWS region must be a standard aws-partition region");
    }
    Ok(())
}

// Bound every read and refuse special files before opening them (including FIFOs/devices).
fn read_file(path: &Path) -> Result<Option<Vec<u8>>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspecting AWS metadata file"),
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        bail!("credentials_unavailable: AWS metadata must be a bounded regular file");
    }
    let file = std::fs::File::open(path).context("opening AWS metadata file")?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading AWS metadata file")?;
    if u64::try_from(bytes.len())? > MAX_FILE_BYTES {
        bail!("credentials_unavailable: AWS metadata file is too large");
    }
    Ok(Some(bytes))
}

/// Reads only the corral-owned binding; OAuth and AWS credential values are never returned.
pub fn read_binding(root: &Path) -> Result<Option<AwsBinding>> {
    let path = auth_dir(root).join(BINDING_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspecting Bedrock binding"),
        Ok(_) => {}
    }
    check_binding_metadata(&auth_dir(root), &path)?;
    let bytes = read_file(&path)?.context("credentials_unavailable: missing Bedrock binding")?;
    // Do not expose serde errors: invalid input can contain secret values.
    let stored: StoredBinding = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("credentials_unavailable: invalid Bedrock binding"))?;
    if stored.kind != "aws_profile" {
        bail!("credentials_unavailable: invalid Bedrock binding type");
    }
    check_profile_name(&stored.profile)?;
    check_region(&stored.region)?;
    Ok(Some(AwsBinding {
        profile: stored.profile,
        region: stored.region,
    }))
}

#[cfg(unix)]
fn check_binding_metadata(dir: &Path, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::fs::symlink_metadata(dir).context("inspecting Bedrock auth directory")?;
    let file = std::fs::symlink_metadata(path).context("inspecting Bedrock binding")?;
    if !directory.is_dir() || directory.permissions().mode() & 0o777 != 0o700 {
        bail!("credentials_unsafe: Bedrock auth must be a 0700 directory");
    }
    if !file.is_file() || file.permissions().mode() & 0o777 != 0o600 {
        bail!("credentials_unsafe: Bedrock binding must be a 0600 regular file");
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_binding_metadata(_dir: &Path, _path: &Path) -> Result<()> {
    bail!("the Pi-native host runs on Unix only")
}

fn write_binding(root: &Path, binding: AwsBinding) -> Result<()> {
    let stored = StoredBinding {
        kind: "aws_profile".to_owned(),
        profile: binding.profile,
        region: binding.region,
    };
    let bytes = serde_json::to_vec(&stored).context("encoding Bedrock binding")?;
    let dir = auth_dir(root);
    ensure_auth_dir(&dir)?;
    // NamedTempFile is created 0600 on Unix; persist atomically replaces even a stale symlink.
    let mut temporary =
        tempfile::NamedTempFile::new_in(&dir).context("creating private Bedrock binding")?;
    temporary
        .write_all(&bytes)
        .context("writing Bedrock binding")?;
    temporary
        .persist(dir.join(BINDING_FILE))
        .map_err(|error| error.error)
        .context("installing Bedrock binding")?;
    Ok(())
}

#[derive(Clone, Debug)]
enum Mechanism {
    Static,
    Sso {
        profile: String,
        expires_at: Option<Timestamp>,
    },
}

/// Non-secret profile metadata only. No access key, session token or SSO token is retained.
#[derive(Clone, Debug)]
pub struct ProfileStatus {
    pub configured_region: Option<String>,
    role_steps: usize,
    mechanism: Mechanism,
}

impl ProfileStatus {
    pub fn check_ready(&self, now: Timestamp) -> Result<()> {
        match &self.mechanism {
            Mechanism::Static => Ok(()),
            Mechanism::Sso {
                profile: _,
                expires_at: Some(expiry),
            } if now < *expiry => Ok(()),
            Mechanism::Sso { profile, .. } => bail!(
                "credentials_unavailable: SSO token missing, invalid or expired; run \
                 `aws sso login --profile {profile}`"
            ),
        }
    }

    pub fn description(&self) -> String {
        let base = match &self.mechanism {
            Mechanism::Static => "static keys".to_owned(),
            Mechanism::Sso {
                expires_at: Some(expiry),
                ..
            } => format!("sso, token valid until {expiry}"),
            Mechanism::Sso {
                expires_at: None, ..
            } => "sso, token unavailable".to_owned(),
        };
        if self.role_steps == 0 {
            base
        } else {
            format!("assume-role ({} steps), {base}", self.role_steps)
        }
    }
}

type Section = BTreeMap<String, String>;
type Ini = BTreeMap<String, Section>;

fn read_ini(path: &Path) -> Result<Ini> {
    let Some(bytes) = read_file(path)? else {
        return Ok(Ini::new());
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("credentials_unavailable: invalid AWS INI encoding"))?;
    let mut sections = Ini::new();
    let mut section = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let name = name.trim().to_owned();
            sections.entry(name.clone()).or_default();
            section = Some(name);
        } else if let (Some(name), Some((key, value))) = (&section, line.split_once('=')) {
            sections
                .entry(name.clone())
                .or_default()
                .insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
        } else {
            // Never include the offending line in diagnostics.
            bail!("credentials_unavailable: malformed AWS INI file");
        }
    }
    Ok(sections)
}

fn present<'a>(section: &'a Section, key: &str) -> Option<&'a str> {
    section
        .get(key)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
}

fn required<'a>(section: &'a Section, key: &str) -> Result<&'a str> {
    present(section, key)
        .with_context(|| format!("credentials_unavailable: AWS profile requires {key}"))
}

fn refuse_unsupported(section: &Section) -> Result<()> {
    for key in [
        "credential_process",
        "web_identity_token_file",
        "credential_source",
        "mfa_serial",
        "login_session",
        "endpoint_url",
        "services",
    ] {
        if section.contains_key(key) {
            bail!("credentials_unavailable: unsupported AWS profile key {key}");
        }
    }
    for key in ["use_fips_endpoint", "use_dualstack_endpoint"] {
        if present(section, key).is_some_and(|value| !value.eq_ignore_ascii_case("false")) {
            bail!("network_refused: unsupported AWS profile key {key}");
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct TokenExpiry {
    #[serde(rename = "expiresAt")]
    expires_at: String,
}

fn sso_expiry(home: &Path, cache_key: &str) -> Result<Option<Timestamp>> {
    let filename = format!("{}.json", sha1_smol::Sha1::from(cache_key).digest());
    let Some(bytes) = read_file(&home.join(".aws/sso/cache").join(filename))? else {
        return Ok(None);
    };
    // Parse only expiresAt; malformed token JSON must never be printed.
    let Ok(token) = serde_json::from_slice::<TokenExpiry>(&bytes) else {
        return Ok(None);
    };
    let expiry = token
        .expires_at
        .strip_suffix("UTC")
        .map(|s| format!("{s}Z"))
        .unwrap_or(token.expires_at);
    Ok(expiry.parse().ok())
}

/// Inspect only `$HOME/.aws/{config,credentials}` and the selected SSO cache file, offline.
pub fn inspect_profile(home: &Path, profile: &str) -> Result<ProfileStatus> {
    check_profile_name(profile)?;
    let config = read_ini(&home.join(".aws/config"))?;
    let credentials = read_ini(&home.join(".aws/credentials"))?;
    let mut seen = BTreeSet::new();
    let mut current = profile.to_owned();
    let mut configured_region = None;
    let mut role_steps = 0;
    loop {
        check_profile_name(&current)?;
        if !seen.insert(current.clone()) {
            bail!("credentials_unavailable: cycle in AWS source_profile chain");
        }
        let config_name = if current == "default" {
            current.clone()
        } else {
            format!("profile {current}")
        };
        let configured = config.get(&config_name);
        let keys = credentials.get(&current);
        if configured.is_none() && keys.is_none() {
            bail!(
                "credentials_unavailable: AWS profile {current} is missing; run \
                 `aws configure --profile {current}`"
            );
        }
        // Credentials-file entries override config keys, as with shared AWS profiles.
        let mut section = configured.cloned().unwrap_or_default();
        if let Some(keys) = keys {
            // A disallowed config key cannot be hidden by a credentials override.
            refuse_unsupported(keys)?;
            section.extend(keys.clone());
        }
        if let Some(configured) = configured {
            refuse_unsupported(configured)?;
        }
        refuse_unsupported(&section)?;
        if role_steps == 0 {
            configured_region = present(&section, "region").map(str::to_owned);
        }
        if let Some(region) = present(&section, "region") {
            check_region(region)?;
        }
        let sso = section.keys().any(|key| key.starts_with("sso_"));
        let static_keys = section.contains_key("aws_access_key_id")
            || section.contains_key("aws_secret_access_key")
            || section.contains_key("aws_session_token");
        if section.contains_key("role_arn") {
            required(&section, "role_arn")?;
            if sso || static_keys {
                bail!("credentials_unavailable: ambiguous AWS assume-role credentials");
            }
            role_steps += 1;
            if role_steps > 4 {
                bail!("credentials_unavailable: AWS source_profile exceeds four assume-role steps");
            }
            if let Some(duration) = present(&section, "duration_seconds")
                && duration
                    .parse::<u32>()
                    .ok()
                    .is_none_or(|n| !(900..=43200).contains(&n))
            {
                bail!("credentials_unavailable: invalid AWS duration_seconds");
            }
            current = required(&section, "source_profile")?.to_owned();
            continue;
        }
        if section.contains_key("source_profile") {
            bail!("credentials_unavailable: source_profile requires role_arn");
        }
        let mechanism = if sso {
            if static_keys {
                bail!("credentials_unavailable: ambiguous AWS SSO and static credentials");
            }
            required(&section, "sso_account_id")?;
            required(&section, "sso_role_name")?;
            let cache_key = if let Some(session) = present(&section, "sso_session") {
                let session_section = config
                    .get(&format!("sso-session {session}"))
                    .context("credentials_unavailable: missing AWS sso-session section")?;
                refuse_unsupported(session_section)?;
                required(session_section, "sso_start_url")?;
                check_region(required(session_section, "sso_region")?)?;
                session
            } else {
                check_region(required(&section, "sso_region")?)?;
                required(&section, "sso_start_url")?
            };
            Mechanism::Sso {
                profile: current,
                expires_at: sso_expiry(home, cache_key)?,
            }
        } else {
            required(&section, "aws_access_key_id")?;
            required(&section, "aws_secret_access_key")?;
            Mechanism::Static
        };
        return Ok(ProfileStatus {
            configured_region,
            role_steps,
            mechanism,
        });
    }
}

const LOGIN_USAGE: &str = "usage_error: Bedrock login requires --aws-profile <name> and \
    --region <region> without a terminal";

/// Explicit-root, noninteractive login for offline callers and fixture tests.
pub fn login_in(root: &Path, home: &Path, profile: &str, region: &str) -> Result<()> {
    login_with_io(
        root,
        home,
        Some(profile),
        Some(region),
        false,
        &mut std::io::empty(),
        &mut std::io::sink(),
    )
}

fn prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: Option<&str>,
) -> Result<String> {
    match default {
        Some(value) => write!(output, "{label} [{value}]: "),
        None => write!(output, "{label}: "),
    }
    .context("writing AWS login prompt")?;
    output.flush().context("flushing AWS login prompt")?;
    let mut line = String::new();
    if input
        .read_line(&mut line)
        .context("reading AWS login response")?
        == 0
    {
        bail!("usage_error: AWS login input ended; no binding was written");
    }
    let line = line.trim();
    Ok(if line.is_empty() {
        default.unwrap_or_default().to_owned()
    } else {
        line.to_owned()
    })
}

/// Rust-only Bedrock login; never runs Bun, contacts AWS or writes beneath `.aws`.
pub fn login(
    root: &Path,
    home: &Path,
    profile: Option<&str>,
    region: Option<&str>,
    terminal: bool,
) -> Result<()> {
    login_with_io(
        root,
        home,
        profile,
        region,
        terminal,
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )
}

fn login_with_io(
    root: &Path,
    home: &Path,
    profile: Option<&str>,
    region: Option<&str>,
    terminal: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<()> {
    if !terminal && (profile.is_none() || region.is_none()) {
        bail!(LOGIN_USAGE);
    }
    let profile = match profile {
        Some(profile) => profile.to_owned(),
        None => prompt(input, output, "AWS profile", None)?,
    };
    let status = inspect_profile(home, &profile)?;
    status.check_ready(Timestamp::now())?;
    let region = match region {
        Some(region) => region.to_owned(),
        None => prompt(
            input,
            output,
            "AWS region",
            status.configured_region.as_deref(),
        )?,
    };
    check_region(&region)?;
    write_binding(root, AwsBinding { profile, region })
}

#[cfg(test)]
#[path = "aws_profile_tests.rs"]
mod tests;

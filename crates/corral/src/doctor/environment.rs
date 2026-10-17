//! Informational comparisons only: never execute anything inside a pane.

use anyhow::{Context, Result, bail};
use corral_core::VERIFICATION_ENV as RELEVANT_ENV_KEYS;
use corral_herdr::Herdr;

#[derive(Debug, PartialEq, Eq)]
struct Environment {
    values: [Option<Vec<u8>>; RELEVANT_ENV_KEYS.len()],
}

impl Environment {
    fn from_lookup(lookup: impl Fn(&str) -> Option<Vec<u8>>) -> Self {
        Self {
            values: RELEVANT_ENV_KEYS.map(lookup),
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_lookup(|key| {
            bytes.split(|byte| *byte == 0).find_map(|entry| {
                entry
                    .strip_prefix(key.as_bytes())?
                    .strip_prefix(b"=")
                    .map(<[u8]>::to_vec)
            })
        })
    }

    fn differences(&self, other: &Self) -> Vec<&'static str> {
        RELEVANT_ENV_KEYS
            .iter()
            .zip(self.values.iter().zip(&other.values))
            .filter_map(|(key, (current, pane))| (current != pane).then_some(*key))
            .collect()
    }
}

#[cfg(target_os = "linux")]
fn process_environment(pid: u32) -> Result<Environment> {
    use std::io::Read;

    // procfs reports size zero; cap the read instead of trusting metadata.
    const LIMIT: u64 = 1024 * 1024;
    let path = format!("/proc/{pid}/environ");
    let file = std::fs::File::open(&path).with_context(|| format!("opening {path}"))?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {path}"))?;
    if bytes.is_empty() || u64::try_from(bytes.len())? > LIMIT {
        bail!("process environment is empty or exceeds the snapshot limit");
    }
    Ok(Environment::from_bytes(&bytes))
}

#[cfg(not(target_os = "linux"))]
fn process_environment(_pid: u32) -> Result<Environment> {
    bail!("read-only process environment snapshots require Linux procfs");
}

pub(super) fn report(herdr: &Herdr) {
    let current = Environment::from_lookup(|key| {
        std::env::var_os(key).map(|value| value.as_encoded_bytes().to_vec())
    });
    report_with(herdr, &current, process_environment);
}

fn report_with(herdr: &Herdr, current: &Environment, read: impl Fn(u32) -> Result<Environment>) {
    let panes = match herdr.pane_list() {
        Ok(panes) => panes,
        Err(error) => {
            corral_ops::report_message(
                None,
                "doctor environment",
                format!("Environment comparison unavailable: {error:#}"),
            );
            return;
        }
    };
    for pane in panes {
        let result = (|| -> Result<()> {
            let info = herdr.pane_process_info(&pane.pane_id)?;
            // A foreground process inherits exported shell changes that /proc's
            // shell launch snapshot cannot show. With an idle shell, only that
            // launch snapshot is available; do not inject an `env` command.
            let pids = if info.foreground_processes.is_empty() {
                vec![info.shell_pid]
            } else {
                info.foreground_processes.iter().map(|p| p.pid).collect()
            };
            for pid in pids {
                let environment = read(pid)
                    .with_context(|| format!("reading process {pid} launch environment"))?;
                let keys = current.differences(&environment);
                if !keys.is_empty() {
                    corral_ops::report_message(
                        None,
                        "doctor environment",
                        format!(
                            "Warning (informational): current CLI and herdr pane {} process {pid} have different launch environment values for {}. Reopen the pane from the intended environment if needed; no pane was changed.",
                            pane.pane_id,
                            keys.join(", ")
                        ),
                    );
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            corral_ops::report_message(
                None,
                "doctor environment",
                format!(
                    "Environment comparison unavailable for herdr pane {}: {error:#}",
                    pane.pane_id
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests;

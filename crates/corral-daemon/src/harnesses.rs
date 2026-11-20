//! Sample descriptive host facts off the daemon event loop and Settings read path.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    io::{ErrorKind, Read},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use corral_engine::{Config, config::ModelSelector};
use corral_protocol::rows::{
    HarnessFact, HarnessStatus, PiChildProfile, PiConfigFacts, PiConfigStatus,
};
use rustix::fs::{self, Access, AtFlags, Mode, OFlags};
use serde::Deserialize;

use crate::state::Shared;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const PI_FILE_MAX: u64 = 256 * 1024;
const FACT_BYTES_MAX: usize = 8 * 1024;
const ENTRIES_MAX: usize = 64;

#[derive(Default)]
pub(crate) struct Inventory(Mutex<Observation>);

#[derive(Default, PartialEq, Eq)]
struct Observation {
    harnesses: Option<(String, Vec<HarnessFact>)>,
    pi: Option<PiConfigFacts>,
}

// A stale mount can block filesystem access indefinitely. Run only one sampler, hold no
// daemon reference during I/O, and never join it as part of shutdown.
pub(crate) fn start_sampler(shared: &Arc<Shared>) {
    let root = shared.root().to_owned();
    let weak = Arc::downgrade(shared);
    if let Err(error) = thread::Builder::new()
        .name("corrald-inventory".into())
        .spawn(move || {
            loop {
                let sample = Observation::read(&root);
                let Some(shared) = weak.upgrade() else { break };
                if shared.lock().stop {
                    break;
                }
                shared.harness_inventory.update(&shared, sample);
                shared.wait(REFRESH_INTERVAL);
                if shared.lock().stop {
                    break;
                }
            }
        })
    {
        eprintln!("corrald harness discovery unavailable: {error}");
    }
}

impl Observation {
    fn read(root: &Path) -> Self {
        let search = std::env::var_os("PATH");
        let harnesses = crate::config::get_at(root).ok().and_then(|doc| {
            let revision = doc.revision?;
            let config = Config::parse(&doc.toml).ok()?;
            Some((revision, harness_facts(&config, search.as_deref())?))
        });
        let path = root.join(crate::STATE_DIR).join("pi.toml");
        Self {
            harnesses,
            pi: pi_config(&path),
        }
    }
}

impl Inventory {
    fn update(&self, shared: &Shared, observation: Observation) {
        let mut previous = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if *previous != observation {
            *previous = observation;
            shared.publish();
        }
    }

    pub(crate) fn facts(
        &self,
        revision: Option<&str>,
    ) -> (Option<Vec<HarnessFact>>, Option<PiConfigFacts>) {
        let observed = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let harnesses = observed
            .harnesses
            .as_ref()
            .and_then(|(sample, facts)| (Some(sample.as_str()) == revision).then(|| facts.clone()));
        (harnesses, observed.pi.clone())
    }
}

fn harness_facts(config: &Config, search: Option<&OsStr>) -> Option<Vec<HarnessFact>> {
    if config.harnesses.len() > ENTRIES_MAX {
        return None;
    }
    let facts: Vec<_> = config
        .harnesses
        .iter()
        .filter_map(|(name, harness)| {
            let name = display_value(name, 256)?.to_owned();
            let program = literal_program(&harness.command);
            let (status, resolved_path) = program
                .map(|value| locate(value, search))
                .unwrap_or((HarnessStatus::Unknown, None));
            Some(HarnessFact {
                name,
                status,
                executable: program.map(str::to_owned),
                resolved_path,
            })
        })
        .collect();
    fits(&facts).then_some(facts)
}

// A harness is a shell template, not an argv array. Only inspect an unambiguous
// first word; do not guess the target of assignments, substitutions or wrappers.
fn literal_program(command: &str) -> Option<&str> {
    let word = command
        .trim_start_matches([' ', '\t'])
        .split([' ', '\t', '\n'])
        .next()?;
    if word.is_empty() || word.len() > 512 {
        return None;
    }
    let word = word
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .or_else(|| {
            word.strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap_or(word);
    (word.starts_with(|ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_'))
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._+-".contains(&b))
        && !matches!(word, "." | ".." | "env" | "exec" | "command"))
    .then_some(word)
}

fn locate(program: &str, search: Option<&OsStr>) -> (HarnessStatus, Option<String>) {
    use HarnessStatus::{Installed, NotFound, Unknown};
    let known = |path: &Path| {
        let status = executable_status(path);
        (
            status,
            (status == Installed)
                .then(|| display_value(path.to_str()?, 1024).map(str::to_owned))
                .flatten(),
        )
    };
    let path = Path::new(program);
    if path.is_absolute() {
        return known(path);
    }
    // Relative commands and PATH components resolve from a run's cwd, not the daemon's.
    let Some(search) = search.filter(|_| !program.contains('/')) else {
        return (Unknown, None);
    };
    let mut status = NotFound;
    for dir in std::env::split_paths(search) {
        if dir.is_absolute() {
            match known(&dir.join(program)) {
                found @ (Installed, _) => return found,
                (NotFound, _) => continue,
                (Unknown, _) => {}
            }
        }
        status = Unknown;
    }
    (status, None)
}

fn executable_status(path: &Path) -> HarnessStatus {
    use HarnessStatus::{Installed, NotFound, Unknown};
    match path.metadata() {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return NotFound,
        Err(error) if absent(&error) => return NotFound,
        Err(_) => return Unknown,
    }
    match fs::accessat(fs::CWD, path, Access::EXEC_OK, AtFlags::EACCESS) {
        Ok(()) => Installed,
        Err(rustix::io::Errno::ACCESS | rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => {
            NotFound
        }
        Err(_) => Unknown,
    }
}

fn absent(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PiSettings {
    build: ChildProfiles,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ChildProfiles {
    profiles: BTreeMap<String, ChildProfile>,
}

#[derive(Deserialize)]
struct ChildProfile {
    model: ModelSelector,
    effort: Option<String>,
    // Pi refuses this old field rather than inferring a connection from it.
    connection: Option<serde::de::IgnoredAny>,
}

fn pi_config(path: &Path) -> Option<PiConfigFacts> {
    let display = display_value(path.to_str()?, 1024)?.to_owned();
    let mut facts = PiConfigFacts {
        path: display,
        status: PiConfigStatus::Unavailable,
        child_profiles: Vec::new(),
    };
    // Nonblocking open also avoids hanging the daemon on a misconfigured FIFO.
    let file = match fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(rustix::io::Errno::NOENT) => {
            facts.status = PiConfigStatus::Missing;
            return Some(facts);
        }
        Err(_) => return Some(facts),
    };
    if let Some(profiles) = read_child_profiles(file) {
        facts.child_profiles = profiles;
        facts.status = PiConfigStatus::Read;
    }
    Some(facts)
}

fn read_child_profiles(file: File) -> Option<Vec<PiChildProfile>> {
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut content = String::new();
    file.take(PI_FILE_MAX + 1)
        .read_to_string(&mut content)
        .ok()?;
    if content.len() as u64 > PI_FILE_MAX {
        return None;
    }
    // Decode only these named fields; no raw TOML, errors, provider settings, or credentials
    // leave this module. Pi remains authoritative on provider/catalog/launch validity.
    let settings: PiSettings = toml::from_str(&content).ok()?;
    if settings.build.profiles.len() > ENTRIES_MAX {
        return None;
    }
    let profiles: Vec<_> = settings
        .build
        .profiles
        .into_iter()
        .map(|(name, row)| {
            if row.connection.is_some() {
                return None;
            }
            let model = display_value(row.model.as_str(), 256)?;
            let provider = model.split_once('/')?.0;
            if !provider
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
            {
                return None;
            }
            let effort = row.effort.filter(|value| !value.is_empty());
            if effort
                .as_ref()
                .is_some_and(|value| display_value(value, 64).is_none())
            {
                return None;
            }
            Some(PiChildProfile {
                name: display_value(&name, 96)?.to_owned(),
                model: model.to_owned(),
                effort,
            })
        })
        .collect::<Option<_>>()?;
    fits(&profiles).then_some(profiles)
}

fn display_value(value: &str, max: usize) -> Option<&str> {
    (!value.is_empty() && value.len() <= max && !value.chars().any(char::is_control))
        .then_some(value)
}

fn fits(value: &impl serde::Serialize) -> bool {
    crate::views::json_len(value).is_ok_and(|length| length <= FACT_BYTES_MAX)
}

#[cfg(test)]
#[path = "harnesses_tests.rs"]
mod tests;

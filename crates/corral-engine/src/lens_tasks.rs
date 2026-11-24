//! Create ordinary authoring tasks for checked-in behaviour lenses.

use std::fmt::Write;

use anyhow::Result;
use corral_facts::Revision;
use corral_types::lenses::{Lens, LensResult, LensStatus};

use crate::{
    Engine, Status, Task, config::shell_quote, facts::FactsReader, refuse, tasks::NewTask,
};

const FORMAT: &str = "Use a unique `id`, a descriptive `title`, and the full Git commit ID of the code you inspected as `read_at`. Every state, transition, participant and message must have a source anchor verified against that code. An `anchor` has a repository-relative `path` and may include a definition `name` and/or literal `text` from that file (within the named definition when known). All `from` and `to` values must refer to nodes declared in the same file.";

const STATE_FORMAT: &str = r#"`states_from` must point to the source enum that defines the states: use its repository-relative `path` and enum `name`. Include every enum variant, using exact variant names (or their Rust snake_case equivalents); don't invent extra states. A transition may include a plain-text `guard`.

```toml
id = "job-lifecycle"
title = "Job lifecycle"
kind = "state"
read_at = "<full-commit-id>"
states_from = { path = "src/job.rs", name = "JobStatus" }

[[states]]
id = "queued"
label = "Queued"
anchor = { path = "src/job.rs", name = "JobStatus::Queued" }

[[states]]
id = "running"
label = "Running"
anchor = { path = "src/job.rs", name = "JobStatus::Running" }

[[transitions]]
from = "queued"
to = "running"
label = "Start job"
anchor = { path = "src/job.rs", text = "job.start()" }
```"#;

const SEQUENCE_FORMAT: &str = r#"List messages in execution order. A message may include a plain-text `note`. If known, name the entry point using `entry = { kind, name }` with a `command`, `route` or `operation` kind.

```toml
id = "job-submission"
title = "Job submission"
kind = "sequence"
read_at = "<full-commit-id>"
entry = { kind = "command", name = "submit" }

[[participants]]
id = "cli"
label = "CLI"
anchor = { path = "src/cli.rs" }

[[participants]]
id = "worker"
label = "Worker"
anchor = { path = "src/worker.rs" }

[[messages]]
from = "cli"
to = "worker"
label = "Submit job"
anchor = { path = "src/cli.rs", text = "worker.submit(job)" }
```"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LensKind {
    State,
    Sequence,
}

impl LensKind {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "state" => Some(Self::State),
            "sequence" => Some(Self::Sequence),
            _ => None,
        }
    }

    fn from_lens(lens: &Lens) -> Self {
        match lens {
            Lens::State { .. } => Self::State,
            Lens::Sequence { .. } => Self::Sequence,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Sequence => "sequence",
        }
    }

    fn template(self) -> String {
        let example = match self {
            Self::State => STATE_FORMAT,
            Self::Sequence => SEQUENCE_FORMAT,
        };
        format!(
            "## Lens format\n\n{FORMAT}\n\n{example}\n\n\
             ## Acceptance\n\nCommit the lens change, then run `corral-next lens check --project <project> --at <task-head-sha> <lens-file>` \
             using the full commit ID of the task worktree's HEAD and the changed lens file. \
             `lens check` must pass; fix failed checks and report any checks that could not run separately. \
             Submit only the lens file change for review; do not change any other files."
        )
    }

    fn draft_key(self, about: &str) -> String {
        format!("draft:{}:{}", self.name(), about.trim())
    }
}

impl Engine {
    pub(crate) fn draft_lens(
        &self,
        project: &str,
        kind: &str,
        about: &str,
        ready: bool,
    ) -> Result<Task> {
        let kind = LensKind::parse(kind)
            .ok_or_else(|| refuse("lens kind must be state or sequence", vec![]))?;
        let about = about.trim();
        if about.is_empty() {
            return Err(refuse("lens draft requires a nonempty subject", vec![]));
        }
        let reader = FactsReader::new(&self.root, &self.config, project)?;
        let dir = reader.lens_dir();
        let keys = [kind.draft_key(about)];
        self.store.transaction(|store| {
            if let Some(task) = store.open_lens_task(project, &keys)? {
                return Ok(task);
            }
            let subject = about.lines().next().unwrap_or(about);
            let title = format!("Draft {} lens: {subject}", kind.name());
            let body = format!(
                "Draft a {} lens about the following behaviour:\n\n{about}\n\n\
                 Inspect the project code and add a single `.toml` lens under {dir:?}. \
                 Choose a new ID and path; do not overwrite an existing lens.\n\n{}",
                kind.name(),
                kind.template(),
            );
            self.insert_new_task(store, new_lens_task(project, title, body, ready), &keys)
        })
    }

    pub(crate) fn repair_lens(&self, project: &str, id: &str, ready: bool) -> Result<Task> {
        let reader = FactsReader::new(&self.root, &self.config, project)?;
        let tip = reader.project_tip(reader.base_branch()?.as_deref())?;
        let rev = reader.resolve_revision(&tip)?;
        let (results, provider) = reader.check_lenses_cached(&rev)?;
        let dir_prefix = format!("{}/", reader.lens_dir());
        let identities = results
            .iter()
            .map(|result| LensIdentity::read(&reader, &rev, result))
            .collect::<Result<Vec<_>>>()?;
        let selected = select_lens(&results, &identities, &dir_prefix, id).ok_or_else(|| {
            let reason = if shared_id(&identities, id) {
                format!("lens ID {id:?} is ambiguous at {tip}; use the lens file path")
            } else {
                format!("no lens {id:?} at {tip}; use corral-next lens list to find its ID or file")
            };
            refuse(reason, vec![])
        })?;
        let result = &results[selected];
        let lens = identities[selected].as_ref().ok_or_else(|| {
            let diagnostic = result
                .failures
                .first()
                .map_or("unknown error", |failure| &failure.message);
            refuse(
                format!(
                    "cannot identify the ID and kind of malformed lens {:?}: {diagnostic}",
                    result.file
                ),
                vec![],
            )
        })?;
        let duplicate_id = shared_id(&identities, &lens.id);
        let repair_key = if duplicate_id {
            format!("repair-file:{:?}:{:?}", lens.id, result.file)
        } else {
            format!("repair:{}", lens.id)
        };
        let mut draft_keys = Vec::new();
        // Link only exact, unambiguous IDs, paths, or titles, not free-form subjects.
        for alias in [
            lens.id.as_str(),
            &result.file,
            result
                .file
                .strip_prefix(&dir_prefix)
                .unwrap_or(&result.file),
            lens.title.trim(),
        ] {
            if alias.is_empty() || shared_id(&identities, alias) {
                continue;
            }
            let points_to_result = match select_lens(&results, &identities, &dir_prefix, alias) {
                Some(found) => found == selected,
                None => !identities.iter().enumerate().any(|(index, other)| {
                    index != selected
                        && other.as_ref().is_some_and(|other| {
                            other.kind == lens.kind && other.title.trim() == alias
                        })
                }),
            };
            if points_to_result {
                draft_keys.push(lens.kind.draft_key(alias));
            }
        }
        let provider_unavailable = provider.is_some_and(|report| report.error.is_some());
        let unchecked = unchecked_diagnostics(result, provider_unavailable)?;
        self.store.transaction(|store| {
            let repair_keys = [repair_key];
            if let Some(task) = store.open_lens_task(project, &repair_keys)? {
                return Ok(task);
            }
            if result.status == LensStatus::Current {
                let mut reason = format!("lens {:?} has no failed checks at {tip}{unchecked}", lens.id);
                if provider_unavailable {
                    let check = check_command(project, &tip, &result.file)?;
                    write!(reason, "\nRun `{check}` to prime the facts provider cache before retrying lens repair.")?;
                }
                return Err(refuse(reason, vec![]));
            }
            let title = if lens.title.trim().is_empty() {
                format!("Repair lens: {}", lens.id)
            } else {
                format!("Repair lens: {} ({})", lens.title, lens.id)
            };
            let id_instruction = if duplicate_id || lens.id.chars().any(|c| c.is_whitespace() || c.is_control()) {
                "Correct its invalid or duplicate ID in place."
            } else {
                "Keep its ID."
            };
            let mut body = format!(
                "Repair the existing {} lens {:?} in {:?}. {id_instruction} Fix the failed checks \
                 and verify its anchors and elements against the code; do not add a replacement lens.\n\n\
                 ## Failed checks at {tip} ({})\n",
                lens.kind.name(),
                lens.id,
                result.file,
                result.status.as_str(),
            );
            for failure in &result.failures {
                writeln!(body, "- {}: {}", failure.element, failure.message)?;
            }
            for state in &result.missing_states {
                writeln!(body, "- Missing state: {state}")?;
            }
            for state in &result.extra_states {
                writeln!(body, "- Extra state: {state}")?;
            }
            write!(body, "{unchecked}\n\n{}", lens.kind.template())?;
            let new = new_lens_task(project, title, body, ready);
            let draft = store.open_lens_task(project, &draft_keys)?;
            let keys = repair_keys.into_iter().chain(draft_keys).collect::<Vec<_>>();
            if let Some(task) = draft {
                self.retarget_lens_draft(store, task, new, &keys)
            } else {
                self.insert_new_task(store, new, &keys)
            }
        })
    }
}

struct LensIdentity {
    id: String,
    title: String,
    kind: LensKind,
}

impl LensIdentity {
    fn read(reader: &FactsReader<'_>, rev: &Revision, result: &LensResult) -> Result<Option<Self>> {
        if let Some(lens) = &result.lens {
            return Ok(Some(Self {
                id: lens.id().to_owned(),
                title: lens.title().to_owned(),
                kind: LensKind::from_lens(lens),
            }));
        }
        if result.failures.iter().any(|failure| {
            matches!(
                failure.message.as_str(),
                "lens file exceeded the lens scan budget" | "expected a regular UTF-8 TOML file"
            )
        }) {
            return Ok(None);
        }
        Ok(reader.lens_header(rev, &result.file)?.and_then(|table| {
            let get = |key| table.get(key).and_then(toml::Value::as_str);
            let id = get("id").filter(|id| !id.trim().is_empty())?;
            Some(Self {
                id: id.to_owned(),
                title: get("title").unwrap_or_default().to_owned(),
                kind: LensKind::parse(get("kind")?)?,
            })
        }))
    }
}

fn shared_id(identities: &[Option<LensIdentity>], id: &str) -> bool {
    identities
        .iter()
        .flatten()
        .filter(|lens| lens.id == id)
        .nth(1)
        .is_some()
}

fn select_lens(
    results: &[LensResult],
    identities: &[Option<LensIdentity>],
    dir_prefix: &str,
    selector: &str,
) -> Option<usize> {
    (!shared_id(identities, selector))
        .then(|| {
            identities
                .iter()
                .position(|lens| lens.as_ref().is_some_and(|lens| lens.id == selector))
        })
        .flatten()
        .or_else(|| results.iter().position(|result| result.file == selector))
        .or_else(|| {
            results
                .iter()
                .position(|result| result.file.strip_prefix(dir_prefix) == Some(selector))
        })
}

fn check_command(project: &str, tip: &str, file: &str) -> Result<String> {
    let word = |value: &str| {
        if value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'))
        {
            Ok(value.to_owned())
        } else {
            shell_quote(value)
        }
    };
    let project_flag = if project.starts_with('-') {
        "--project="
    } else {
        "--project "
    };
    let separator = if file.starts_with('-') { "-- " } else { "" };
    Ok(format!(
        "corral-next lens check {project_flag}{} --at {tip} {separator}{}",
        word(project)?,
        word(file)?
    ))
}

fn unchecked_diagnostics(result: &LensResult, provider_unavailable: bool) -> Result<String> {
    if !provider_unavailable && result.not_checked.is_empty() {
        return Ok(String::new());
    }
    let mut text = String::from("\n\n## Not checked\n");
    if provider_unavailable {
        text.push_str("- No cached facts provider answer was available for this revision; repair did not start an extension.\n");
    }
    for skipped in &result.not_checked {
        writeln!(text, "- {}: {}", skipped.element, skipped.message)?;
    }
    Ok(text)
}

fn new_lens_task(project: &str, title: String, body: String, ready: bool) -> NewTask {
    NewTask {
        title,
        body,
        project: Some(project.to_owned()),
        status: if ready {
            Status::Ready
        } else {
            Status::Backlog
        },
        ..NewTask::default()
    }
}

#[cfg(test)]
#[path = "lens_tasks_tests.rs"]
mod tests;

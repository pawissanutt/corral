//! Create ordinary authoring tasks for checked-in behaviour lenses.

use std::fmt::Write;

use anyhow::Result;
use corral_types::lenses::{Lens, LensResult, LensStatus};

use crate::{Engine, Status, Task, facts::FactsReader, refuse, tasks::NewTask};

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
        let kind = match kind {
            "state" => LensKind::State,
            "sequence" => LensKind::Sequence,
            _ => return Err(refuse("lens kind must be state or sequence", vec![])),
        };
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
        let result = select_lens(&results, &dir_prefix, id).ok_or_else(|| {
            refuse(
                format!(
                    "no lens {id:?} at {tip}; use corral-next lens list to find its ID or file"
                ),
                vec![],
            )
        })?;
        let lens = result.lens.as_ref().ok_or_else(|| {
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
        let kind = LensKind::from_lens(lens);
        let mut keys = vec![format!("repair:{}", lens.id())];
        // A draft about this exact ID, path, or unambiguous title is already
        // lens work. Don't guess links from free-form descriptions.
        for alias in [
            lens.id(),
            &result.file,
            result
                .file
                .strip_prefix(&dir_prefix)
                .unwrap_or(&result.file),
            lens.title().trim(),
        ] {
            let points_to_result = match select_lens(&results, &dir_prefix, alias) {
                Some(found) => found.file == result.file,
                None => !results.iter().any(|other| {
                    other.file != result.file
                        && other.lens.as_ref().is_some_and(|other| {
                            LensKind::from_lens(other) == kind && other.title().trim() == alias
                        })
                }),
            };
            if points_to_result {
                keys.push(kind.draft_key(alias));
            }
        }
        let provider_unavailable = provider.is_some_and(|report| report.error.is_some());
        let unchecked = unchecked_diagnostics(result, provider_unavailable)?;
        self.store.transaction(|store| {
            if let Some(task) = store.open_lens_task(project, &keys)? {
                return Ok(task);
            }
            if result.status == LensStatus::Current {
                return Err(refuse(
                    format!(
                        "lens {:?} has no failed checks at {tip}{unchecked}",
                        lens.id()
                    ),
                    vec![],
                ));
            }
            let title = format!("Repair lens: {} ({})", lens.title(), lens.id());
            let mut body = format!(
                "Repair the existing {} lens {:?} in {:?}. Keep its ID and update its anchors \
                 or elements to match the code; do not add a replacement lens.\n\n\
                 ## Failed checks at {tip} ({})\n",
                kind.name(),
                lens.id(),
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
            write!(body, "{unchecked}\n\n{}", kind.template())?;
            self.insert_new_task(store, new_lens_task(project, title, body, ready), &keys)
        })
    }
}

fn select_lens<'a>(
    results: &'a [LensResult],
    dir_prefix: &str,
    selector: &str,
) -> Option<&'a LensResult> {
    results
        .iter()
        .find(|result| {
            result
                .lens
                .as_ref()
                .is_some_and(|lens| lens.id() == selector)
        })
        .or_else(|| results.iter().find(|result| result.file == selector))
        .or_else(|| {
            results
                .iter()
                .find(|result| result.file.strip_prefix(dir_prefix) == Some(selector))
        })
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

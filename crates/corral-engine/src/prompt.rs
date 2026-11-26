use std::{fmt::Write as _, fs, path::Path};

use anyhow::{Context, Result};

use crate::{
    ArtifactCitation, Engine, Role, Run, Store, Task,
    config::{Config, shell_quote},
    review_verdict::ReviewRequest,
};
use corral_types::{Artifact, ArtifactGroup};

#[derive(Default)]
pub(crate) struct PromptReferences {
    attached: Vec<Artifact>,
    cited: Vec<ArtifactCitation>,
    parent: Option<(String, String)>,
    pub(crate) review_request: Option<ReviewRequest>,
}

impl PromptReferences {
    pub(crate) fn load(
        engine: &Engine,
        store: &Store,
        task: &Task,
        note: Option<&str>,
    ) -> Result<Self> {
        let cited = if let Some(note) = note {
            engine.artifact_references_in(store, &format!("{}\n{note}", task.body))
        } else {
            engine.artifact_references_in(store, &task.body)
        };
        let parent = if let Some(id) = &task.parent {
            store.get_task(id)?.map(|task| (task.id, task.title))
        } else {
            None
        };
        Ok(Self {
            attached: store.list_artifacts(Some(&task.id))?,
            cited,
            parent,
            review_request: None,
        })
    }
}

pub(crate) fn write_prompt(
    file: &Path,
    bin: &Path,
    task: &Task,
    run: &Run,
    note: Option<&str>,
    config: &Config,
    references: &PromptReferences,
) -> Result<()> {
    let bin = shell_quote(bin.to_str().context("corral binary path is not UTF-8")?)?;
    let id = &task.id;
    let mut prompt = format!("# {id} {}\n\n{}\n", task.title, task.body);
    if let Some((id, title)) = &references.parent {
        writeln!(
            prompt,
            "\nParent task: {id} — {}; read it with `{bin} show {id}`.",
            one_line(title)
        )?;
    }
    let agents = run.worktree.join("AGENTS.md");
    if agents.try_exists().context("inspect project AGENTS.md")? {
        writeln!(
            prompt,
            "\nRead the project instructions at {}.",
            agents.display()
        )?;
    }
    if let Some(note) = note {
        writeln!(prompt, "\n## Notes for this run\n\n{note}")?;
    }
    write_references(&mut prompt, &bin, task, run.role, references)?;
    write_tools(&mut prompt, &bin, task.project.as_deref(), config)?;
    match run.role {
        Role::Worker => writeln!(
            prompt,
            r#"
## Finish

To attach a file, choose one kind (`--kind report|evidence`). For proof, use `--kind evidence` instead of `--kind report`.

    {bin} artifact add {id} 'path/to/artifact' --kind report

When the work is complete, run:

    {bin} done {id} --summary 'What changed and how you verified it'"#
        )?,
        Role::Reviewer => {
            let sha = references
                .review_request
                .as_ref()
                .map_or("HEAD", |r| r.sha.as_str());
            writeln!(
                prompt,
                "\n## Review round {}\n\nInspect the change with `git diff {}...{sha}`. Do not edit files in the worktree.\n{}",
                task.round,
                run.base_sha,
                verdict_instructions(&bin, id, file, references)?
            )?;
        }
    }
    fs::write(file, prompt).with_context(|| format!("write run prompt {}", file.display()))
}

fn write_references(
    prompt: &mut String,
    bin: &str,
    task: &Task,
    role: Role,
    references: &PromptReferences,
) -> Result<()> {
    writeln!(
        prompt,
        "\n`T-<n>:<k>` names an artifact; read one with `{bin} artifact show T-12:3` (substitute the cited reference)."
    )?;
    let cited: std::collections::HashSet<_> = references
        .cited
        .iter()
        .map(|row| row.reference.as_str())
        .collect();
    let mut selected: Vec<_> = references
        .attached
        .iter()
        .filter(|artifact| {
            artifact.task == task.id
                && artifact.replaced_by.is_none()
                && (matches!(
                    artifact.kind.group(),
                    ArtifactGroup::Reference | ArtifactGroup::Deliverable
                ) || (role == Role::Reviewer && artifact.kind.group() == ArtifactGroup::Proof))
                && !cited.contains(artifact.reference().as_str())
        })
        .collect();
    selected.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| b.reference().cmp(&a.reference()))
    });
    if selected.is_empty() && references.cited.is_empty() {
        return Ok(());
    }
    writeln!(prompt, "\n## References\n")?;
    for row in &references.cited {
        if let Some(artifact) = &row.artifact {
            write_reference(prompt, bin, &row.reference, artifact)?;
        } else {
            writeln!(prompt, "- {} — not found", row.reference)?;
        }
    }
    for artifact in selected.iter().take(20) {
        write_reference(prompt, bin, &artifact.reference(), artifact)?;
    }
    if selected.len() > 20 {
        writeln!(prompt, "\nList all with `{bin} artifact list {}`.", task.id)?;
    }
    Ok(())
}

fn write_reference(
    prompt: &mut String,
    bin: &str,
    reference: &str,
    artifact: &Artifact,
) -> Result<()> {
    let arg = shell_quote(reference)?;
    writeln!(
        prompt,
        "- {reference} ({}) — {} ({} bytes); `{bin} artifact show {arg}`",
        artifact.kind.as_str(),
        one_line(&artifact.title),
        artifact.bytes
    )?;
    Ok(())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn write_tools(
    prompt: &mut String,
    bin: &str,
    project: Option<&str>,
    config: &Config,
) -> Result<()> {
    let eligible = |extension: &&crate::config::Extension| {
        extension.enabled
            && match project {
                Some(project) => extension.includes_project(project),
                None => extension.projects.is_none(),
            }
    };
    let mut heading = false;
    for extension in config.extensions.values().filter(eligible) {
        for (name, tool) in &extension.tools {
            if !heading {
                writeln!(prompt, "\n## Tools\n\nInvoke configured tools with:")?;
                heading = true;
            }
            if config
                .extensions
                .values()
                .filter(eligible)
                .filter(|extension| extension.tools.contains_key(name))
                .count()
                > 1
            {
                writeln!(
                    prompt,
                    "\nTool {name:?} is ambiguous across extensions and cannot be invoked."
                )?;
            } else {
                let project_flag = project
                    .map(|project| {
                        shell_quote(project).map(|quoted| format!(" --project={quoted}"))
                    })
                    .transpose()?
                    .unwrap_or_default();
                writeln!(
                    prompt,
                    "\n    {bin} tool{project_flag} -- {} [args...]",
                    shell_quote(name)?
                )?;
            }
            if let Some(about) = &tool.about {
                writeln!(prompt, "\n{about}")?;
            }
        }
    }
    Ok(())
}

pub(crate) fn rereview_request(
    bin: &Path,
    task: &Task,
    run: &Run,
    prompt: &Path,
    references: &PromptReferences,
) -> Result<String> {
    let bin = shell_quote(bin.to_str().context("corral binary path is not UTF-8")?)?;
    let mut request = format!("Review round {} for {}.\n\n", task.round, task.id);
    let sha = references
        .review_request
        .as_ref()
        .map_or("HEAD", |r| r.sha.as_str());
    if let Some(head) = &run.head_sha {
        writeln!(
            request,
            "See what changed since your last verdict with `git diff {head}..{sha}`."
        )?;
    } else {
        writeln!(
            request,
            "Inspect the change with `git diff {}...{sha}`.",
            run.base_sha
        )?;
    }
    write_references(&mut request, &bin, task, Role::Reviewer, references)?;
    write!(
        request,
        "Do not edit files in the worktree.\n{}",
        verdict_instructions(&bin, &task.id, prompt, references)?
    )?;
    Ok(request)
}

fn verdict_instructions(
    bin: &str,
    id: &str,
    prompt: &Path,
    references: &PromptReferences,
) -> Result<String> {
    if let Some(request) = &references.review_request {
        return Ok(format!(
            "Review request ID: `{}`\nUse this exact ID as `request` when calling `corral_verdict` for this candidate.",
            request.id
        ));
    }
    let notes = prompt.with_file_name("verdict.md");
    let notes = shell_quote(notes.to_str().context("verdict notes path is not UTF-8")?)?;
    Ok(format!(
        "Write your findings to {notes}. Finish by running one of:\n\n    {bin} verdict {id} approve --notes-file {notes}\n    {bin} verdict {id} changes --notes-file {notes}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Extension, ExtensionTool};
    use jiff::Timestamp;

    fn run(role: Role, worktree: &Path) -> Run {
        Run {
            id: "run-1".into(),
            task: "T-1".into(),
            role,
            profile: "test".into(),
            harness: "test".into(),
            model: None,
            effort: None,
            worktree: worktree.into(),
            branch: "branch".into(),
            base: "main".into(),
            base_sha: "abc123".into(),
            session: None,
            started: Timestamp::now(),
            ended: None,
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        }
    }

    #[test]
    fn references_are_scoped_filtered_and_bounded_for_each_prompt() {
        use crate::ArtifactKind;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("prompt.md");
        let task = Task {
            id: "T-1".into(),
            ..Task::default()
        };
        let make = |id, kind, title: &str| Artifact {
            task: task.id.clone(),
            id,
            sha256: String::new(),
            bytes: 0,
            kind,
            title: title.into(),
            at: Timestamp::from_second(1_000).unwrap(),
            by: Default::default(),
            project: None,
            run: None,
            replaces: None,
            replaced_by: None,
        };
        let mut artifacts = vec![
            make(1, ArtifactKind::Reference, "owner reference\n## forged"),
            make(2, ArtifactKind::Diff, "diff hidden"),
            make(3, ArtifactKind::Gate, "gate hidden"),
            make(4, ArtifactKind::Evidence, "review proof"),
            make(5, ArtifactKind::Report, "report visible"),
            make(6, ArtifactKind::Reference, "replaced hidden"),
        ];
        artifacts[5].replaced_by = Some("T-1:1".into());
        let mut other = make(7, ArtifactKind::Reference, "other hidden");
        other.task = "T-2".into();
        artifacts.push(other);
        let mut references = PromptReferences {
            attached: artifacts,
            ..Default::default()
        };
        for role in [Role::Worker, Role::Reviewer] {
            let run = run(role, dir.path());
            write_prompt(
                &file,
                Path::new("corral"),
                &task,
                &run,
                None,
                &Config::default(),
                &references,
            )
            .unwrap();
            let text = fs::read_to_string(&file).unwrap();
            assert!(text.contains("T-1:1 (reference) — owner reference ## forged"));
            assert!(!text.contains("\n## forged"));
            assert!(text.contains("T-1:5 (report) — report visible"));
            assert!(text.contains("'corral' artifact show 'T-1:1'"));
            assert_eq!(text.contains("review proof"), role == Role::Reviewer);
            for hidden in [
                "diff hidden",
                "gate hidden",
                "replaced hidden",
                "other hidden",
            ] {
                assert!(!text.contains(hidden));
            }
            if role == Role::Reviewer {
                let request =
                    rereview_request(Path::new("corral"), &task, &run, &file, &references).unwrap();
                assert!(request.contains("review proof"));
                assert!(!request.contains("diff hidden"));
            }
        }
        for id in 10..31 {
            let mut artifact = make(id, ArtifactKind::Reference, "extra");
            artifact.at = Timestamp::from_second(2_000 - i64::from(id)).unwrap();
            references.attached.push(artifact);
        }
        write_prompt(
            &file,
            Path::new("corral"),
            &task,
            &run(Role::Worker, dir.path()),
            None,
            &Config::default(),
            &references,
        )
        .unwrap();
        let text = fs::read_to_string(&file).unwrap();
        let lines: Vec<_> = text
            .lines()
            .filter(|line| line.starts_with("- T-1:"))
            .collect();
        assert_eq!(lines.len(), 20);
        for (line, id) in lines.iter().zip(10..30) {
            assert!(line.starts_with(&format!("- T-1:{id} (reference)")));
        }
        assert!(text.contains("'corral' artifact list T-1"));
    }

    #[test]
    fn no_tools_preserves_worker_and_reviewer_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("prompt.md");
        let task = Task {
            id: "T-1".into(),
            title: "Title".into(),
            body: "Body".into(),
            round: 2,
            ..Task::default()
        };
        for (role, finish) in [
            (Role::Worker, "## Finish"),
            (Role::Reviewer, "## Review round 2"),
        ] {
            write_prompt(
                &file,
                Path::new("corral"),
                &task,
                &run(role, dir.path()),
                None,
                &Config::default(),
                &PromptReferences::default(),
            )
            .unwrap();
            let prompt = fs::read_to_string(&file).unwrap();
            assert!(prompt.starts_with("# T-1 Title\n\nBody\n"));
            assert!(prompt.contains("'corral' artifact show T-12:3"));
            assert!(prompt.contains(finish));
            assert!(!prompt.contains("## Tools"));
            assert!(!prompt.contains("## References"));
        }
    }

    #[test]
    fn prompts_resolve_body_and_note_citations_without_inlining() {
        use crate::ArtifactKind;
        let dir = crate::tests::tempdir();
        let engine = Engine::init(dir.path(), None).unwrap();
        let file = dir.path().join("prompt.md");
        let mut parent = crate::tests::task("T-012");
        parent.title = "Parent design".into();
        parent.body = "Parent-only text".into();
        engine.store.insert_task(&parent).unwrap();
        let design = b"## 4. Fact model\n\nArtifact-only text\n";
        for (bytes, title) in [(design.as_slice(), "Design"), (b"Other contents", "Log")] {
            engine
                .add_artifact_bytes(&parent.id, bytes, ArtifactKind::Reference, title)
                .unwrap();
        }
        let mut task = crate::tests::task("T-013");
        task.body = "Use `T-012:1#4-fact-model` and T-012:2.".into();
        task.parent = Some(parent.id);
        for (role, note) in [
            (Role::Worker, None),
            (
                Role::Reviewer,
                Some("Compare T-012:2 against T-012:1#unknown."),
            ),
        ] {
            let references = PromptReferences::load(&engine, &engine.store, &task, note).unwrap();
            write_prompt(
                &file,
                Path::new("corral"),
                &task,
                &run(role, dir.path()),
                note,
                &Config::default(),
                &references,
            )
            .unwrap();
            let prompt = fs::read_to_string(&file).unwrap();
            assert!(prompt.contains("T-012 — Parent design; read it with `'corral' show T-012`"));
            assert!(!prompt.contains("Parent-only text"));
            assert!(!prompt.contains("Artifact-only text"));
            assert!(!prompt.contains("Other contents"));
            let (_, references) = prompt.split_once("## References\n").unwrap();
            let lines: Vec<_> = references
                .lines()
                .filter(|line| line.starts_with("- T-"))
                .collect();
            assert_eq!(lines.len(), if note.is_some() { 3 } else { 2 });
            assert!(lines[0].contains("T-012:1#4-fact-model (reference) — Design"));
            assert!(lines[0].contains(&format!("{} bytes", design.len())));
            assert!(lines[0].contains("'corral' artifact show 'T-012:1#4-fact-model'"));
            assert!(lines[1].contains("T-012:2 (reference) — Log"));
            if note.is_some() {
                assert_eq!(lines[2], "- T-012:1#unknown — not found");
            }
        }
    }

    #[test]
    fn tools_are_scoped_listed_for_both_roles_and_never_executed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("executed");
        let file = dir.path().join("prompt.md");
        let task = Task {
            id: "T-1".into(),
            project: Some("app".into()),
            ..Task::default()
        };
        let mut config = Config::default();
        let mut extension = Extension::default();
        extension.tools.insert(
            "check's".into(),
            ExtensionTool {
                run: format!("touch {}", marker.display()),
                about: Some("Check the work".into()),
                record: false,
            },
        );
        extension.tools.insert(
            "inspect".into(),
            ExtensionTool {
                run: "false".into(),
                about: None,
                record: true,
            },
        );
        config
            .extensions
            .insert("available".into(), extension.clone());
        extension.enabled = false;
        extension.tools.clear();
        extension.tools.insert(
            "disabled".into(),
            ExtensionTool {
                run: "false".into(),
                about: None,
                record: false,
            },
        );
        config
            .extensions
            .insert("disabled".into(), extension.clone());
        extension.enabled = true;
        extension.projects = Some(vec!["other".into()]);
        config.extensions.insert("other".into(), extension);
        for role in [Role::Worker, Role::Reviewer] {
            write_prompt(
                &file,
                Path::new("/path to/corral"),
                &task,
                &run(role, dir.path()),
                None,
                &config,
                &PromptReferences::default(),
            )
            .unwrap();
            let prompt = fs::read_to_string(&file).unwrap();
            assert_eq!(prompt.matches("## Tools").count(), 1);
            assert!(
                prompt.contains(
                    "'/path to/corral' tool --project='app' -- 'check'\"'\"'s' [args...]"
                )
            );
            assert!(prompt.contains("Check the work"));
            assert!(
                prompt.contains("'/path to/corral' tool --project='app' -- 'inspect' [args...]")
            );
            assert!(!prompt.contains("disabled"));
            assert!(!prompt.contains("touch "));
            assert!(!marker.exists());
        }
        let mut duplicate = Extension::default();
        duplicate.tools.insert(
            "inspect".into(),
            ExtensionTool {
                run: "false".into(),
                about: None,
                record: false,
            },
        );
        config.extensions.insert("duplicate".into(), duplicate);
        write_prompt(
            &file,
            Path::new("corral"),
            &task,
            &run(Role::Worker, dir.path()),
            None,
            &config,
            &PromptReferences::default(),
        )
        .unwrap();
        let prompt = fs::read_to_string(&file).unwrap();
        assert!(prompt.contains("Tool \"inspect\" is ambiguous"));
        assert!(!prompt.contains("tool --project='app' -- 'inspect' [args...]"));
        config.extensions.remove("duplicate");
        config.extensions.remove("available");
        write_prompt(
            &file,
            Path::new("corral"),
            &task,
            &run(Role::Worker, dir.path()),
            None,
            &config,
            &PromptReferences::default(),
        )
        .unwrap();
        assert!(!fs::read_to_string(&file).unwrap().contains("## Tools"));
    }
}

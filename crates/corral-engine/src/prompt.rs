use std::{fmt::Write as _, fs, path::Path};

use anyhow::{Context, Result};

use crate::{
    Role, Run, Task,
    config::{Config, shell_quote},
};

pub(crate) fn write_prompt(
    file: &Path,
    bin: &Path,
    task: &Task,
    run: &Run,
    note: Option<&str>,
    config: &Config,
) -> Result<()> {
    let bin = shell_quote(bin.to_str().context("corral binary path is not UTF-8")?)?;
    let id = &task.id;
    let mut prompt = format!("# {id} {}\n\n{}\n", task.title, task.body);
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
    write_tools(&mut prompt, &bin, task.project.as_deref(), config)?;
    match run.role {
        Role::Worker => writeln!(
            prompt,
            r#"
## Finish

Attach any reports or evidence using:

    {bin} artifact add {id} 'path/to/artifact'

When the work is complete, run:

    {bin} done {id} --summary 'What changed and how you verified it'"#
        )?,
        Role::Reviewer => writeln!(
            prompt,
            "\n## Review round {}\n\nInspect the change with `git diff {}...HEAD`. Do not edit files in the worktree.\n{}",
            task.round,
            run.base_sha,
            verdict_commands(&bin, id, file)?
        )?,
    }
    fs::write(file, prompt).with_context(|| format!("write run prompt {}", file.display()))
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
) -> Result<String> {
    let bin = shell_quote(bin.to_str().context("corral binary path is not UTF-8")?)?;
    let mut request = format!("Review round {} for {}.\n\n", task.round, task.id);
    if let Some(head) = &run.head_sha {
        writeln!(
            request,
            "See what changed since your last verdict with `git diff {head}..HEAD`."
        )?;
    } else {
        writeln!(
            request,
            "Inspect the change with `git diff {}...HEAD`.",
            run.base_sha
        )?;
    }
    write!(
        request,
        "Do not edit files in the worktree.\n{}",
        verdict_commands(&bin, &task.id, prompt)?
    )?;
    Ok(request)
}

fn verdict_commands(bin: &str, id: &str, prompt: &Path) -> Result<String> {
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
        let config = Config::default();
        write_prompt(
            &file,
            Path::new("corral"),
            &task,
            &run(Role::Worker, dir.path()),
            None,
            &config,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "# T-1 Title\n\nBody\n\n## Finish\n\nAttach any reports or evidence using:\n\n    'corral' artifact add T-1 'path/to/artifact'\n\nWhen the work is complete, run:\n\n    'corral' done T-1 --summary 'What changed and how you verified it'\n"
        );
        write_prompt(
            &file,
            Path::new("corral"),
            &task,
            &run(Role::Reviewer, dir.path()),
            None,
            &config,
        )
        .unwrap();
        let expected = format!(
            "# T-1 Title\n\nBody\n\n## Review round 2\n\nInspect the change with `git diff abc123...HEAD`. Do not edit files in the worktree.\n{}\n",
            verdict_commands("'corral'", "T-1", &file).unwrap()
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), expected);
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
        )
        .unwrap();
        assert!(!fs::read_to_string(&file).unwrap().contains("## Tools"));
    }
}

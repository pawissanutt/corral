use std::{fmt::Write as _, fs, path::Path};

use anyhow::{Context, Result};

use crate::{Role, Run, Task, config::shell_quote};

pub(crate) fn write_prompt(
    file: &Path,
    bin: &Path,
    task: &Task,
    run: &Run,
    note: Option<&str>,
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

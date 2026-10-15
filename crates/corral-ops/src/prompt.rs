use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{ProfileChoice, ResolvedProfile, Run, Task};

use crate::Ctx;

/// The dispatch prompt for a run, as markdown. Contains, in order:
/// 1. "# <id> <title>" and the task body verbatim;
/// 2. "Project: <key> at <worktree>; branch <branch> from <base>. Follow <project>/AGENTS.md if it exists.";
/// 3. a "## When you finish" section instructing EXACTLY:
///    `<corral> done <id> --summary "<one paragraph: what changed and how it was verified>"` and
///    `<corral> artifact add <id> <path> --kind <report|plan|evidence|screenshot|log> --title "<title>"` for
///    every report/plan/screenshot worth keeping, where <corral> is the absolute path of the running binary
///    (`std::env::current_exe()`), plus "Commit your work on the branch before calling done.";
/// 4. "If you need a decision from the user, ask it plainly and stop; corral will surface it on the board."
pub fn task_prompt(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    let corral = corral_bin()?;
    let corral = corral.display();
    let key = task.project.as_deref().unwrap_or("(none)");
    let project_dir = match task.project.as_deref() {
        Some(p) => ctx.store.project_path(p)?,
        None => run.worktree.clone(),
    };
    let id = &task.id;
    let mut out = format!("# {id} {}\n\n", task.title);
    out.push_str(&task.body);
    if !task.body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&format!(
        "Project: {key} at {}; branch {} from {}. Follow {}/AGENTS.md if it exists.\n\n",
        run.worktree.display(),
        run.branch,
        run.base,
        project_dir.display()
    ));
    out.push_str("## When you finish\n\n");
    out.push_str("Commit your work on the branch before calling done.\n\n");
    out.push_str(&format!(
        "For every report/plan/screenshot worth keeping, run:\n\n    {corral} artifact add {id} <path> --kind <report|plan|evidence|screenshot|log> --title \"<title>\"\n\n"
    ));
    out.push_str(&format!(
        "Then run:\n\n    {corral} done {id} --summary \"<one paragraph: what changed and how it was verified>\"\n\n"
    ));
    out.push_str("If you need a decision from the user, ask it plainly and stop; corral will surface it on the board.\n");
    Ok(out)
}

/// Renders the command template of profile `profile` (see `AgentCmd` placeholders) for a run, with the run's
/// model/effort as overrides. Unknown profile is an error listing configured profiles. Single quotes inside
/// substituted values are escaped for sh ('\'').
pub fn render_command(
    ctx: &Ctx,
    profile: &str,
    task: &Task,
    run: &Run,
    prompt_file: &Path,
) -> Result<String> {
    let choice = ProfileChoice {
        profile: Some(profile.to_string()),
        model: run.model.clone(),
        effort: run.effort.clone(),
    };
    let resolved = ctx.store.config.resolve_profile(&choice, None, None)?;
    let name = format!("{} {}", task.id, task.title);
    render_template(
        &resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", name),
            ("task", task.id.clone()),
            ("worktree", run.worktree.display().to_string()),
        ],
    )
}

pub(crate) fn corral_bin() -> Result<PathBuf> {
    std::env::current_exe().context("resolving the corral binary path (current_exe)")
}

pub(crate) fn render_template(
    resolved: &ResolvedProfile,
    values: &[(&str, String)],
) -> Result<String> {
    let corral = corral_bin()?.display().to_string();
    let mut out = if resolved.model_args.is_empty() {
        resolved
            .command
            .replace(" {model_args}", "")
            .replace("{model_args}", "")
    } else {
        let args: Vec<String> = resolved.model_args.iter().map(|a| sh_quote(a)).collect();
        resolved.command.replace("{model_args}", &args.join(" "))
    };
    out = out.replace("{corral}", &sh_escape(&corral));
    for (key, value) in values {
        out = out.replace(&format!("{{{key}}}"), &sh_escape(value));
    }
    Ok(out)
}

fn sh_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// A standalone sh word: bare when every char is safe, else single-quoted.
fn sh_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if safe {
        value.to_string()
    } else {
        format!("'{}'", sh_escape(value))
    }
}

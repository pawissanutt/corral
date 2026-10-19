use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{Config, ExecutionSnapshot, ProfileChoice, ResolvedProfile, Run, Task};

use crate::Ctx;

const RUN_TEMPLATE: &str = include_str!("../assets/run.md");

/// The dispatch prompt for a run, as markdown, rendered from the run template: the run harness's
/// `[harnesses.<k>] prompt` file (relative to the workspace root, or absolute), else `.corral/prompts/run.md`,
/// else the built-in `assets/run.md` (task header and body, then the run contract). Placeholders:
/// {{id}} {{title}} {{body}} {{project}} {{project_dir}} {{worktree}} {{branch}} {{base}} {{agents_md}}
/// {{gate}} {{artifact_cmd}} {{artifact_globs}} {{done_cmd}} {{corral}}. Commands use the absolute path of the
/// running binary. The gate is the effective `[review]` gate, else the gate quoted in the repo's AGENTS.md.
pub fn task_prompt(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    let template = run_template(ctx, run)?;
    let corral = shell_path(&corral_bin()?);
    let project_dir = match task.project.as_deref() {
        Some(p) => ctx.store.project_path(p)?,
        None => run.worktree.clone(),
    };
    let id = &task.id;
    let body = if task.body.trim().is_empty() {
        "(no spec beyond the title)".to_string()
    } else {
        task.body.trim_end().to_string()
    };
    let agents_md = [&run.worktree, &project_dir]
        .into_iter()
        .map(|d| d.join("AGENTS.md"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| project_dir.join("AGENTS.md"));
    let policy = ctx
        .store
        .config
        .review_policy(task.project.as_deref(), task.review);
    let size_limits = serde_json::to_string(&crate::integration::effective_size_limits(
        ctx,
        task,
        agents_md.parent().unwrap_or(&project_dir),
        &policy,
    )?)?;
    let gate = if policy.gate.is_empty() {
        quoted_gate(&agents_md)?.map(|cmds| ("quoted in AGENTS.md", cmds))
    } else {
        Some(("corral's [review] gate", policy.gate))
    };
    let gate = match gate {
        Some((source, cmds)) => {
            let lines: Vec<String> = std::iter::once(format!(
                "       export CORRAL_SIZE_LIMITS='{}'",
                sh_escape(&size_limits)
            ))
            .chain(
                policy
                    .env
                    .iter()
                    .map(|(name, value)| format!("       export {name}='{}'", sh_escape(value))),
            )
            .chain(cmds.iter().map(|c| format!("       {c}")))
            .collect();
            format!(
                "The gate ({source}), run from the worktree root:\n\n{}",
                lines.join("\n")
            )
        }
        None => "No gate is configured; run the checks the repo's AGENTS.md asks for.".to_string(),
    };
    let globs: Vec<String> = ctx
        .store
        .config
        .artifacts
        .globs
        .iter()
        .map(|g| format!("`{g}`"))
        .collect();
    let values = [
        ("id", id.clone()),
        ("title", task.title.clone()),
        ("body", body),
        (
            "project",
            task.project.clone().unwrap_or_else(|| "(none)".into()),
        ),
        ("project_dir", project_dir.display().to_string()),
        ("worktree", run.worktree.display().to_string()),
        ("branch", run.branch.clone()),
        ("base", run.base.clone()),
        ("agents_md", agents_md.display().to_string()),
        ("gate", gate),
        (
            "artifact_cmd",
            format!(
                "{corral} artifact add {id} <path> --kind <report|plan|evidence|screenshot|log> --title \"<title>\""
            ),
        ),
        ("artifact_globs", globs.join(", ")),
        (
            "done_cmd",
            format!(
                "{corral} done {id} --summary \"<activity and verification>\" --result \"<cumulative task outcome>\""
            ),
        ),
        ("corral", corral.clone()),
    ];
    let mut prompt = fill(&template, &values);
    if let Some(link) = &task.plan
        && let Some((parent, revision)) = link.split_once('@')
    {
        let parent = ctx.store.get(parent)?;
        let plan = parent
            .plans
            .iter()
            .find(|p| p.id == revision)
            .context("linked approved plan revision is missing")?;
        prompt.push_str(&format!(
            "\n## Approved design plan\n\nPlan: {link}\nSnapshot: {}\nSHA-256: {}\n\nRead this approved plan before implementing the task's own scope.\n",
            ctx.store.dir().join(&plan.artifact.path).display(), plan.artifact.sha256,
        ));
    }
    prompt.push_str(&crate::lessons::lessons_prompt(ctx, task, run)?);
    prompt.push_str(&crate::comments::owner_comments(task));
    let packet = crate::resume_packet(ctx, task, run)?;
    prompt.push_str("\n## Portable resume contract\n\n");
    prompt.push_str("The task specification above is canonical. Progress is not criterion acceptance. \
        Reuse only compatible progress; stale checkpoints are historical context, not valid checks. \
        Pending waits and resource cleanup remain obligations even when the tree changed. \
        Before explicit handoff or a reachable retry, commit your work, read \
        `corral run progress <task> --json`, then write \
        `corral run checkpoint <task> --file <json>` with the returned identity and revisions. \
        Adapters may request checkpoints before compaction; corral does not intercept context limits.\n\n");
    prompt.push_str("<corral-resume>\n");
    prompt.push_str(&serde_json::to_string_pretty(&packet)?);
    prompt.push_str("\n</corral-resume>\n");
    append_working_instructions(&mut prompt, run.execution.as_ref());
    Ok(prompt)
}

pub(crate) fn append_working_instructions(
    prompt: &mut String,
    snapshot: Option<&ExecutionSnapshot>,
) {
    if let Some(snapshot) = snapshot
        && !snapshot.instructions.trim().is_empty()
    {
        prompt.push_str("\n## Configured working instructions\n\n");
        prompt.push_str(&snapshot.instructions);
        prompt.push('\n');
    }
}

pub(crate) fn execution_snapshot(
    ctx: &Ctx,
    resolved: ResolvedProfile,
) -> Result<ExecutionSnapshot> {
    let template = harness_template(ctx, Some(&resolved.harness))?;
    let mut snapshot = ExecutionSnapshot::new(&ctx.store.config, resolved);
    snapshot.prompt_template = Some(template);
    Ok(snapshot)
}

/// The per-harness template, else `.corral/prompts/run.md`, else the built-in one.
fn run_template(ctx: &Ctx, run: &Run) -> Result<String> {
    if let Some(snapshot) = &run.execution {
        return Ok(snapshot
            .prompt_template
            .as_deref()
            .unwrap_or(RUN_TEMPLATE)
            .to_string());
    }
    harness_template(ctx, run.harness.as_deref())
}

fn harness_template(ctx: &Ctx, harness: Option<&str>) -> Result<String> {
    let harness_prompt = harness
        .and_then(|k| ctx.store.config.harnesses.get(k))
        .and_then(|h| h.prompt.as_deref());
    if let Some(path) = harness_prompt {
        let file = ctx.store.root().join(path);
        return std::fs::read_to_string(&file)
            .with_context(|| format!("reading harness prompt template {}", file.display()));
    }
    let file = ctx.store.prompts_dir().join("run.md");
    match std::fs::read_to_string(&file) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(RUN_TEMPLATE.to_string()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// The gate commands an AGENTS.md quotes: the first inline code span on a line whose text before it mentions
/// "gate", or the fenced block right after a line that mentions "gate" and ends with ':'. None when the file
/// is missing or quotes no gate.
fn quoted_gate(agents_md: &Path) -> Result<Option<Vec<String>>> {
    let text = match std::fs::read_to_string(agents_md) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", agents_md.display())),
    };
    Ok(parse_quoted_gate(&text))
}

fn parse_quoted_gate(text: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if let Some((head, tail)) = line.split_once('`')
            && head.to_ascii_lowercase().contains("gate")
            && let Some((cmd, _)) = tail.split_once('`')
            && !cmd.trim().is_empty()
        {
            return Some(vec![cmd.trim().to_string()]);
        }
        if line.to_ascii_lowercase().contains("gate") && line.trim_end().ends_with(':') {
            let mut rest = lines[i + 1..].iter().skip_while(|l| l.trim().is_empty());
            if rest
                .next()
                .is_some_and(|l| l.trim_start().starts_with("```"))
            {
                let cmds: Vec<String> = rest
                    .take_while(|l| !l.trim_start().starts_with("```"))
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_string)
                    .collect();
                if !cmds.is_empty() {
                    return Some(cmds);
                }
            }
        }
    }
    None
}

/// Replaces `{{key}}` placeholders in one pass, so substituted values are never re-expanded. Unknown keys stay.
pub(crate) fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let key = after[..end].trim();
        match values.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => out.push_str(v),
            None => out.push_str(&rest[start..start + end + 4]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
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
    let resolved = match &run.execution {
        Some(snapshot) => snapshot.resolved.clone(),
        None => ctx.store.config.resolve_profile(&choice, None, None)?,
    };
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

/// Task id the settings preview renders commands for.
pub const PREVIEW_TASK: &str = "T-042";

/// The command a dispatch of `profile` would send to its pane under `config` (which may be an unsaved draft),
/// rendered for a sample run R1 of task T-042 "Sample task" in the first configured project. Errors like
/// `resolve_profile` does.
pub fn preview_command(ctx: &Ctx, config: &Config, profile: &str) -> Result<String> {
    let choice = ProfileChoice {
        profile: Some(profile.to_string()),
        ..ProfileChoice::default()
    };
    let resolved = config.resolve_profile(&choice, None, None)?;
    let root = ctx.store.root();
    let repo = config
        .projects
        .values()
        .next()
        .map_or_else(|| root.to_path_buf(), |p| root.join(&p.path));
    let worktree = repo.join(".worktrees").join(PREVIEW_TASK);
    let prompt_file = ctx
        .store
        .prompts_dir()
        .join(format!("{PREVIEW_TASK}-R1.md"));
    render_template(
        &resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", format!("{PREVIEW_TASK} Sample task")),
            ("task", PREVIEW_TASK.to_string()),
            ("worktree", worktree.display().to_string()),
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
    let corral = shell_path(&corral_bin()?);
    // No adapter is enabled in phase 1. Keep the slot optional for all legacy templates.
    corral_core::validate_delivery_template(&resolved.command, false)?;
    let command = resolved
        .command
        .replace(" {delivery_args}", "")
        .replace("{delivery_args}", "");
    let mut out = if resolved.model_args.is_empty() {
        command
            .replace(" {model_args}", "")
            .replace("{model_args}", "")
    } else {
        let args: Vec<String> = resolved.model_args.iter().map(|a| sh_quote(a)).collect();
        command.replace("{model_args}", &args.join(" "))
    };
    out = out.replace("{corral}", &sh_escape(&corral));
    for (key, value) in values {
        let value = match *key {
            "prompt_file" | "worktree" => shell_path(Path::new(value)),
            _ => value.clone(),
        };
        out = out.replace(&format!("{{{key}}}"), &sh_escape(&value));
    }
    Ok(out)
}

pub(crate) fn sh_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// A path usable by the configured POSIX shell, including Git Bash on Windows.
pub(crate) fn shell_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return format!("//{}", unc.replace('\\', "/"));
        }
        value.trim_start_matches(r"\\?\").replace('\\', "/")
    }
    #[cfg(not(windows))]
    value.into_owned()
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

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{
    Config, ExecutionSnapshot, FULL_TIER, ProfileChoice, ResolvedProfile, Run, Task,
};

use crate::Ctx;
use crate::gate_tier::{DestinationKind, GateContract, GatePhase};

const RUN_TEMPLATE: &str = include_str!("../assets/run.md");

/// The dispatch prompt for a run, as markdown, rendered from the run template: the run harness's
/// `[harnesses.<k>] prompt` file (relative to the workspace root, or absolute), else `.corral/prompts/run.md`,
/// else the built-in `assets/run.md` (task header and body, then the run contract). Placeholders:
/// {{id}} {{title}} {{body}} {{project}} {{project_dir}} {{worktree}} {{branch}} {{base}} {{agents_md}}
/// {{gate}} {{artifact_cmd}} {{artifact_globs}} {{done_cmd}} {{corral}} {{skill}} (the worker role skill). Commands use the absolute path of the
/// running binary. The gate is the effective `[review]` gate, else the gate quoted in the repo's AGENTS.md.
/// A tiered run's `{{gate}}` points to the one authoritative Gate contract section that is always
/// appended after the template, whether or not the template contains `{{gate}}`.
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
    let exports: Vec<String> = std::iter::once(format!(
        "export CORRAL_SIZE_LIMITS='{}'",
        sh_escape(&size_limits)
    ))
    .chain(
        policy
            .env
            .iter()
            .map(|(name, value)| format!("export {name}='{}'", sh_escape(value))),
    )
    .collect();
    let contract = tiered_contract(ctx, task, run)?;
    let gate = if policy.gate.is_empty() {
        quoted_gate(&agents_md)?.map(|cmds| ("quoted in AGENTS.md", cmds))
    } else {
        Some(("corral's [review] gate", policy.gate))
    };
    let gate = match gate {
        _ if contract.is_some() => GATE_POINTER.to_string(),
        Some((source, cmds)) => {
            let lines: Vec<String> = exports
                .iter()
                .chain(&cmds)
                .map(|c| format!("       {c}"))
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
        ("skill", crate::skill::WORKER_SKILL.to_string()),
    ];
    let mut prompt = fill(&template, &values);
    if let Some(contract) = &contract {
        prompt.push_str(&worker_gate_section(contract, &corral, task, run, &exports));
    }
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

const GATE_POINTER: &str =
    "The Gate contract (authoritative) section below is the only gate instruction for this run.";

/// A tiered run's worker contract: the review-phase selection, the tier it gates every round on.
/// None for legacy runs and untiered projects, which keep today's gate text.
pub(crate) fn tiered_contract(ctx: &Ctx, task: &Task, run: &Run) -> Result<Option<GateContract>> {
    if run.gate_rules.is_legacy() {
        return Ok(None);
    }
    let contract = crate::gate_tier::select(ctx, task, run, GatePhase::Review)?;
    Ok((!contract.rules.is_legacy()).then_some(contract))
}

/// Who runs the landing tier, and where.
fn landing_text(contract: &GateContract) -> String {
    let branch = Some(contract.destination.branch.as_str());
    match contract.destination.kind {
        DestinationKind::PlanTarget => plan_target_landing(branch, &contract.landing_tier),
        DestinationKind::Main | DestinationKind::Other => {
            branch_landing(&contract.landing_tier, branch)
        }
    }
}

/// A Plan member's landing; `branch` is None when the caller cannot establish the target.
pub(crate) fn plan_target_landing(branch: Option<&str>, tier: &str) -> String {
    let target = branch.map_or_else(
        || "its Plan target".to_owned(),
        |branch| format!("its Plan target `{branch}`"),
    );
    format!(
        "this member lands on {target} on the {tier} tier, and corral gates the aggregate on the full tier itself at plan verify"
    )
}

/// A main-bound or other-branch landing; `branch` is None when only main-boundness is known.
pub(crate) fn branch_landing(tier: &str, branch: Option<&str>) -> String {
    let on = branch.map_or_else(|| "its main branch".to_owned(), |b| format!("`{b}`"));
    format!("corral itself runs the one {tier} gate when this candidate lands on {on}")
}

/// The re-gate instruction of a send-back for a run on a reduced tier: the tier, the one
/// receipted gate, and what is reported instead of fixed. `landing` says who runs the landing tier.
pub(crate) fn tier_send_back(tier: &str, gate: &str, landing: &str) -> String {
    format!(
        "Then run `{gate}` once after the final rebase: it runs the {tier} tier, and its receipt is the one autopilot and review reuse. \
         Fix failures inside the declared module scope that are not expected failures. Report failures outside that scope, and expected failures, \
         under `Not verified:` and as filed follow-ups instead of fixing them. Do not start, or attach evidence of, a full-tier gate: {landing}."
    )
}

/// The send-back text for `run`, with `gate` as the command to name, when its contract is on a
/// reduced tier: `(tier, text)`. None keeps today's full-gate wording (legacy, untiered, full).
pub(crate) fn reduced_send_back(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    gate: &str,
) -> Result<Option<(String, String)>> {
    Ok(tiered_contract(ctx, task, run)?
        .filter(|contract| contract.tier != FULL_TIER)
        .map(|contract| {
            let text = tier_send_back(&contract.tier, gate, &landing_text(&contract));
            (contract.tier, text)
        }))
}

fn listed(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        return format!(" {empty}");
    }
    items.iter().map(|i| format!("\n  - `{i}`")).collect()
}

/// The one authoritative gate instruction of a tiered worker prompt.
fn worker_gate_section(
    contract: &GateContract,
    corral: &str,
    task: &Task,
    run: &Run,
    exports: &[String],
) -> String {
    let id = &task.id;
    let gate = format!("{corral} gate {id} --wait");
    let commands: String = exports
        .iter()
        .chain(&contract.commands)
        .map(|c| format!("\n      {c}"))
        .collect();
    let expected = if contract.expected_failures.is_empty() {
        " (none)".to_owned()
    } else {
        contract
            .expected_failures
            .iter()
            .map(|f| format!("\n  - `{}`: {}", f.test, f.reason))
            .collect()
    };
    format!(
        "\n## Gate contract (authoritative)\n\n\
         This is the only gate instruction for this run. It takes precedence over any other gate \
         instruction in this prompt, the configured working instructions, lessons, skills or \
         repository guidance such as AGENTS.md.\n\n\
         - Tier: {tier}, for every review round of this run (source: {source})\n\
         - Gate: `{gate}`, exactly once per round, after the final rebase. corral selects the tier, \
         records the receipt and attaches the log; autopilot reuses that receipt, so start no second gate.\n\
         - Order: commit and rebase onto the current `{base}`; run `{gate}`; for an audited task, capture \
         evidence and record it with `{corral} evidence record {id}` against the attempt that gate printed; \
         then `{corral} done {id} --check` and the Run contract's `done` command.\n\
         - Commands the {tier} tier runs (reference only; corral runs them):{commands}\n\
         - Module scope:{scope}\n\
         - Declared tests:{tests}\n\
         - Expected failures:{expected}\n\
         - Landing: {landing}; do not start it yourself.\n\n\
         Rules:\n\n\
         - Run no other tier, no `scripts/gate.sh` and none of the commands above by hand, instead of \
         or in addition to `{gate}`, and attach no evidence of such a run.\n\
         - Fix failures inside the module scope that are not expected failures, commit, and repeat \
         from the rebase.\n\
         - Do not fix a failure outside the module scope, or an expected failure. List it under \
         `Not verified:` in the summary, file a follow-up with the corral-submit skill, and ask in this \
         pane when it keeps your tier from passing; only the owner can amend this gate.\n\
         - If the gate is refused for capacity, retry it, and ask in this pane if it keeps being refused.\n",
        tier = contract.tier,
        source = contract.source,
        base = run.base,
        scope = listed(&contract.scope, "(the task declares no write scope)"),
        tests = listed(&contract.tests, "(none: the tier's own selection)"),
        landing = landing_text(contract),
    )
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
    render_command_noticed(ctx, profile, task, run, prompt_file, None)
}

/// `render_command` whose `{prompt_ref}` message also carries a single-line new-round `notice`.
pub(crate) fn render_command_noticed(
    ctx: &Ctx,
    profile: &str,
    task: &Task,
    run: &Run,
    prompt_file: &Path,
    notice: Option<&str>,
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
    let resolved = crate::role_session::command(ctx, run, &resolved)?;
    let name = format!("{} {}", task.id, task.title);
    let mut values = vec![
        ("prompt_file", prompt_file.display().to_string()),
        ("name", name),
        ("task", task.id.clone()),
        ("worktree", run.worktree.display().to_string()),
    ];
    if let Some(notice) = notice {
        values.push((NOTICE_KEY, notice.to_string()));
    }
    render_template(&resolved, &values)
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
        .replace(INLINE_PROMPT, PROMPT_REF)
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
            "prompt_file" if out.contains("{prompt_file}") => {
                shell_path(&bounded(Path::new(value))?)
            }
            "prompt_file" | "worktree" => shell_path(Path::new(value)),
            _ => value.clone(),
        };
        out = out.replace(&format!("{{{key}}}"), &sh_escape(&value));
    }
    if out.contains(PROMPT_REF) {
        let file = values
            .iter()
            .find(|(key, _)| *key == "prompt_file")
            .context("{prompt_ref} needs a prompt file")?;
        let message = pointer(Path::new(&file.1));
        let message = match values.iter().find(|(key, _)| *key == NOTICE_KEY) {
            Some((_, notice)) => format!("{notice} {message}"),
            None => message,
        };
        out = out.replace(PROMPT_REF, &sh_quote(&message));
    }
    Ok(out)
}

/// Template value naming a new-round notice that precedes the `{prompt_ref}` pointer message.
pub(crate) const NOTICE_KEY: &str = "round_notice";
const PROMPT_REF: &str = "{prompt_ref}";
/// The pre-T-260 default: the whole prompt as one argument, which Linux caps at 128 KiB.
const INLINE_PROMPT: &str = "\"$(cat '{prompt_file}')\"";
/// Above this size `{prompt_file}` names a short pointer file instead of the prompt, so no shell
/// spelling (`$(cat ...)`, fish `(cat ...)`, `read`) can turn the prompt into an argument.
const INLINE_PROMPT_LIMIT: u64 = 64 * 1024;

fn pointer(prompt: &Path) -> String {
    format!(
        "Read {} in full and follow it: that file is your complete prompt for this session.",
        shell_path(prompt)
    )
}

/// The prompt itself while it is small, else a pointer file next to it naming it.
fn bounded(prompt: &Path) -> Result<PathBuf> {
    match std::fs::metadata(prompt) {
        Ok(meta) if meta.len() > INLINE_PROMPT_LIMIT => {}
        Ok(_) => return Ok(prompt.to_path_buf()),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(prompt.to_path_buf()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", prompt.display())),
    }
    let file = prompt.with_extension("pointer.md");
    std::fs::write(&file, format!("{}\n", pointer(prompt)))
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(file)
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

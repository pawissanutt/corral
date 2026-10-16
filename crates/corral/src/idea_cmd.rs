use anyhow::{Context, Result, bail};
use corral_core::{Origin, Status, Store, TaskKind, TaskSize};
use corral_ops::Ctx;

#[derive(clap::Args)]
pub struct IdeaArgs {
    pub id: String,
    #[arg(value_parser = ["drop", "decline", "defer", "narrow", "merge"])]
    pub action: String,
    #[arg(long)]
    pub reason: String,
    /// Who made this decision, independent of who filed the task
    #[arg(long, default_value = "owner", value_parser = ["owner", "pm", "agent"])]
    pub author: String,
    #[arg(long)]
    pub into: Option<String>,
    #[arg(long)]
    pub body_file: Option<std::path::PathBuf>,
    /// Record the owner's explicit permission for this decision
    #[arg(long)]
    pub owner_ok: bool,
}

pub fn parse_origin(value: &str) -> Result<Origin, String> {
    match value {
        "owner" => Ok(Origin::Owner),
        "pm" => Ok(Origin::Pm),
        "agent" => Ok(Origin::Agent),
        _ => Err("origin must be owner, pm or agent".into()),
    }
}

#[expect(clippy::too_many_arguments, reason = "Existing new-task CLI options")]
pub fn new_task(
    ctx: &Ctx,
    title: &[String],
    project: Option<&str>,
    status: Status,
    body: Option<&str>,
    body_file: Option<&std::path::Path>,
    profile: Option<String>,
    depends: &[String],
    priority: Option<String>,
    kind: Option<TaskKind>,
    size: Option<TaskSize>,
    origin: Origin,
) -> Result<()> {
    let body = match (body_file, body) {
        (Some(f), _) => {
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?
        }
        (None, Some(b)) => b.to_string(),
        (None, None) => String::new(),
    };
    let deps = depends
        .iter()
        .map(|d| Store::normalize_id(d))
        .collect::<Result<Vec<_>>>()?;
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let task = corral_ops::new_task_with_origin(
        ctx,
        &title.join(" "),
        project,
        &cwd,
        status,
        &body,
        origin,
    )?;
    ctx.store.update(&task.id, |t| {
        t.profile = profile;
        t.depends_on = deps;
        if let Some(p) = priority {
            t.priority = p.to_ascii_uppercase();
        }
        t.kind = kind.map(|kind| kind.to_string());
        t.size = size;
        Ok(())
    })?;
    println!("{}", task.id);
    Ok(())
}

pub fn run(ctx: &Ctx, args: IdeaArgs) -> Result<()> {
    let body = args
        .body_file
        .as_ref()
        .map(|path| {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        })
        .transpose()?;
    let task = corral_ops::decide_idea(
        ctx,
        &args.id,
        &args.action,
        &args.reason,
        corral_ops::IdeaDecisionOptions {
            into: args.into.as_deref(),
            body: body.as_deref(),
            owner_ok: args.owner_ok,
            actor: &args.author,
        },
    )?;
    println!("{}: {} ({})", task.id, args.action, task.status.label());
    Ok(())
}

/// Edit a scratch copy so the common store boundary can reject an unapproved reduction.
pub fn edit(ctx: &Ctx, id: &str, owner_ok: bool) -> Result<()> {
    let before = ctx.store.get(id)?;
    let path = ctx.store.state_dir().join(format!("edit-{}.md", before.id));
    let _lock = ctx
        .store
        .try_lock(&format!("edit-{}", before.id))?
        .context("another editor is open for this task")?;
    std::fs::create_dir_all(ctx.store.state_dir()).context("creating editor scratch directory")?;
    std::fs::write(&path, corral_core::render_task(&before))
        .context("writing editor scratch copy")?;
    let editor = std::env::var("EDITOR")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "vi".into());
    let mut parts = editor.split_whitespace();
    let prog = parts.next().context("empty $EDITOR")?;
    let status = std::process::Command::new(prog)
        .args(parts)
        .arg(&path)
        .status()
        .with_context(|| format!("running {editor}"))?;
    if !status.success() {
        bail!(
            "{editor} exited with {status}; scratch copy: {}",
            path.display()
        );
    }
    let text = std::fs::read_to_string(&path).context("reading edited task")?;
    let mut edited = corral_core::parse_task(&text, &before.path)?;
    if edited.id != before.id {
        bail!("editing a task cannot change its id");
    }
    if owner_ok {
        ctx.store
            .owner_decision(id, "narrow", "Owner approved edited specification", "flag")?;
    }
    ctx.store.update(id, |fresh| {
        if fresh.updated != before.updated {
            bail!(
                "task changed while editor was open; scratch copy: {}",
                path.display()
            );
        }
        edited.updated = fresh.updated;
        *fresh = edited;
        Ok(())
    })?;
    std::fs::remove_file(&path).context("removing editor scratch copy")?;
    Ok(())
}

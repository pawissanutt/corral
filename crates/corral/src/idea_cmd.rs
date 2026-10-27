use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{Origin, Status, Store, TaskKind, TaskSize};
use corral_ops::Ctx;

#[derive(serde::Serialize, serde::Deserialize)]
struct EditDraft {
    task_revision: u64,
    spec_revision: String,
    text: String,
}

const EDIT_DRAFTS: corral_core::records::RecordArea<EditDraft> =
    corral_core::records::RecordArea::new("edit-drafts");

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
    design: bool,
    labels: &[String],
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
    let task = corral_ops::new_task_initialized(
        ctx,
        &title.join(" "),
        project,
        &cwd,
        status,
        &body,
        |t| {
            t.origin = Some(origin);
            t.profile = profile;
            t.depends_on = deps;
            if let Some(p) = priority {
                t.priority = p.to_ascii_uppercase();
            }
            t.kind = kind.map(|kind| kind.to_string());
            t.size = size;
            t.design = design;
            t.labels = labels.to_vec();
            Ok(())
        },
    )?;
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

/// Edit a scratch copy so the common store boundary can reject an unapproved reduction. The
/// copy names the task and spec revisions it was rendered from; a task that changed while the
/// editor was open is refused and the edited text is kept.
pub fn edit(ctx: &Ctx, id: &str, owner_ok: bool) -> Result<()> {
    let revision = ctx.store.head(id)?.rev;
    let before = ctx.store.get(id)?;
    let spec = corral_core::spec_revision(&before)?;
    let dir = ctx.store.state_dir().join("tmp");
    let path = dir.join(format!("edit-{}.md", before.id));
    let _lock = ctx
        .store
        .try_lock(&format!("edit-{}", before.id))?
        .context("another editor is open for this task")?;
    std::fs::create_dir_all(&dir).context("creating editor scratch directory")?;
    let rendered = corral_core::Store::render_task_markdown(&before);
    let text = rendered.replacen(
        "---\n",
        &format!(
            "---\n# corral edit {}: task revision {revision}, spec revision {spec}\n",
            before.id
        ),
        1,
    );
    let retained = EDIT_DRAFTS.load(&ctx.store, &before.id)?;
    let mut draft = match retained {
        Some(draft) if draft.task_revision == revision && draft.spec_revision == spec => draft,
        retained => {
            if let Some(stale) = retained {
                let key = format!("{}-r{}", before.id, stale.task_revision);
                EDIT_DRAFTS.save(&ctx.store, &key, &stale)?;
                eprintln!(
                    "Previous draft kept in edit-drafts/{key}; starting from task revision {revision}"
                );
            }
            EditDraft {
                task_revision: revision,
                spec_revision: spec.clone(),
                text,
            }
        }
    };
    EDIT_DRAFTS.save(&ctx.store, &before.id, &draft)?;
    std::fs::write(&path, &draft.text).context("writing editor scratch copy")?;
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
    draft.text = std::fs::read_to_string(&path).context("reading edited task")?;
    EDIT_DRAFTS.save(&ctx.store, &before.id, &draft)?;
    if !status.success() {
        bail!(
            "{editor} exited with {status}; scratch copy: {}",
            path.display()
        );
    }
    let mut edited = corral_core::Store::parse_task_markdown(&draft.text)?;
    if edited.id != before.id {
        bail!("editing a task cannot change its id");
    }
    before.check_scope_unchanged(&edited)?;
    if revision != draft.task_revision || spec != draft.spec_revision {
        bail!(
            "task changed since draft revision {}; the edited text is kept in {}",
            draft.task_revision,
            path.display()
        );
    }
    if owner_ok {
        ctx.store
            .owner_decision(id, "narrow", "Owner approved edited specification", "flag")?;
    }
    ctx.store.update(id, |fresh| {
        if fresh.updated != before.updated || corral_core::spec_revision(fresh)? != spec {
            bail!(
                "task changed while editor was open (it was at revision {revision}); the edited text is kept in {}",
                path.display()
            );
        }
        edited.updated = fresh.updated;
        *fresh = edited;
        EDIT_DRAFTS.delete(&ctx.store, &before.id)
    })?;
    std::fs::remove_file(&path).context("removing editor scratch copy")?;
    Ok(())
}

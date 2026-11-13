use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{Origin, Status, TaskDifficulty, TaskKind, TaskSize};
use corral_ops::Ctx;

use crate::invocation::Invocation;

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
    inv: &Invocation,
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
    difficulty: Option<TaskDifficulty>,
    origin: Origin,
    design: bool,
    labels: &[String],
) -> Result<()> {
    use corral_protocol::cmd::{Command, task::TaskCommand};
    let body = match (body_file, body) {
        (Some(f), _) => {
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?
        }
        (None, Some(b)) => b.to_string(),
        (None, None) => String::new(),
    };
    inv.submit(Command::Task(TaskCommand::New {
        title: title.to_vec(),
        origin: origin.as_str().into(),
        project: project.map(str::to_owned),
        status: status.label().to_ascii_lowercase(),
        body,
        profile,
        depends: depends.to_vec(),
        priority,
        kind: kind.map(|v| v.to_string()),
        size: size.map(|v| v.to_string()),
        difficulty: difficulty.map(|v| v.to_string()),
        design,
        labels: labels.to_vec(),
    }))
}

pub fn run(inv: &Invocation, args: IdeaArgs) -> Result<()> {
    use corral_protocol::cmd::{Command, task::TaskCommand};
    let body = args
        .body_file
        .as_ref()
        .map(|path| {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        })
        .transpose()?;
    inv.submit(Command::Task(TaskCommand::Idea {
        id: args.id,
        action: args.action,
        reason: args.reason,
        into: args.into,
        body,
        owner_ok: args.owner_ok,
    }))
}

/// Edit a scratch copy so the common store boundary can reject an unapproved reduction. The
/// copy names the task and spec revisions it was rendered from; a task that changed while the
/// editor was open is refused and the edited text is kept.
pub fn edit(ctx: &Ctx, id: &str, owner_ok: bool) -> Result<()> {
    let revision = ctx.store.head(id)?.rev;
    let before = ctx.store.get(id)?;
    let spec = corral_core::spec_revision(&before)?;
    let dir = ctx.store.scratch_tmp()?;
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

use anyhow::{Context, Result, ensure};
use clap::Args;
use corral_core::{ArtifactKind, ArtifactTier, Store};
use corral_ops::{ContentSearch, Ctx, search_content};

#[derive(Args)]
pub struct SearchArgs {
    /// Shared query language: quoted phrases, negation, test:, error:, verdict:
    #[arg(required_unless_present_any = ["reindex", "catalog_build", "catalog_status"])]
    query: Option<String>,
    /// Content scope; omitted preserves the task-result format
    #[arg(long = "in", value_parser = ["artifacts", "specs", "reviews", "all"])]
    scope: Option<String>,
    #[arg(long)]
    task: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long, value_parser = ["keep", "normal", "transient"])]
    tier: Option<String>,
    /// Updated/added within Nh, Nd, Nw, or after a date or RFC3339 timestamp
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    project: Option<String>,
    /// Maximum hits, capped at 100
    #[arg(long, default_value = "20", value_parser = clap::value_parser!(u32).range(0..=100))]
    limit: u32,
    /// Source lines before and after the matching line, capped at 20
    #[arg(long, default_value = "0", value_parser = clap::value_parser!(u32).range(0..=20))]
    context: u32,
    /// Rebuild the derived index from canonical stored content
    #[arg(long)]
    reindex: bool,
    /// Resume the opt-in complete-source catalog in the foreground
    #[arg(long, conflicts_with_all = ["query", "catalog_status"])]
    catalog_build: bool,
    /// Show durable catalog progress without starting a builder
    #[arg(long, conflicts_with_all = ["query", "catalog_build", "reindex"])]
    catalog_status: bool,
    #[arg(long)]
    json: bool,
}

pub fn run(ctx: &Ctx, args: SearchArgs) -> Result<()> {
    if args.catalog_build || args.catalog_status {
        let status = if args.catalog_build {
            ContentSearch::build_catalog(ctx, args.reindex)?
        } else {
            ContentSearch::catalog_status(ctx)?
        };
        if args.json {
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            println!("catalog: {}", status["state"].as_str().unwrap_or("unknown"));
            println!("cache: {}", status["cache_root"].as_str().unwrap_or(""));
            println!(
                "progress: {}",
                status["progress_file"].as_str().unwrap_or("")
            );
            if let Some(phase) = status["phase"].as_str() {
                println!("phase: {phase}");
            }
            if let Some(error) = status["last_error"].as_str() {
                println!("last error: {error}");
            }
        }
        return Ok(());
    }
    let pinned = Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    let ctx = &pinned;
    let query = args.query.as_deref().unwrap_or("");
    ensure!(query.len() <= 4096, "search query exceeds 4096 bytes");
    let options = ContentSearch {
        scope: args.scope.clone().unwrap_or_else(|| "all".into()),
        task: args.task.as_deref().map(Store::normalize_id).transpose()?,
        kind: args
            .kind
            .as_deref()
            .map(|k| ArtifactKind::parse(k).with_context(|| format!("unknown artifact kind '{k}'")))
            .transpose()?,
        tier: args
            .tier
            .as_deref()
            .map(|t| ArtifactTier::parse(t).context("unknown artifact tier"))
            .transpose()?,
        since: args.since,
        project: args.project,
        limit: usize::try_from(args.limit).context("limit overflow")?,
        context: usize::try_from(args.context).context("context overflow")?,
        reindex: args.reindex,
    };
    let content_mode = args.scope.is_some()
        || options.task.is_some()
        || options.kind.is_some()
        || options.tier.is_some()
        || options.since.is_some()
        || options.project.is_some()
        || options.context != 0
        || args.reindex;
    if content_mode {
        let hits = search_content(ctx, query, &options)?;
        if args.json {
            println!("{}", serde_json::to_string(&hits)?);
        } else {
            for hit in hits {
                println!(
                    "{} [{} {}] {}:{}-{}\n{}",
                    hit.reference,
                    hit.kind,
                    hit.tier.as_str(),
                    hit.task,
                    hit.line_start,
                    hit.line_end,
                    hit.snippet
                );
            }
        }
    } else {
        // Keep T-047's machine-consumed task shape for existing callers.
        let ids = corral_ops::SearchData::search_ids(&ctx.store, query, options.limit)?;
        let _assembly = corral_core::timing::span("search.result_assembly");
        if args.json {
            // Render one keyed task at a time: JSON task rows do not expose full histories.
            let rows: Vec<_> = ids
                .iter()
                .map(|id| {
                    let task = ctx.store.get(id)?;
                    Ok(crate::cmds::task_output::task_json(&task, false))
                })
                .collect::<Result<_>>()?;
            println!("{}", serde_json::to_string(&rows)?);
        } else {
            let tasks = ids
                .iter()
                .map(|id| ctx.store.get(id))
                .collect::<Result<Vec<_>>>()?;
            crate::list_cols::print(ctx, &tasks, &[])?;
        }
    }
    Ok(())
}

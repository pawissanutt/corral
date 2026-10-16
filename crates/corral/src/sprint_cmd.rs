use anyhow::Result;
use corral_ops::Ctx;

#[derive(clap::Args)]
pub struct SprintArgs {
    /// Use the last review mark (also the default)
    #[arg(value_parser = ["last"], conflicts_with = "since")]
    pub window: Option<String>,
    /// Cover events strictly after this RFC3339 timestamp, or the last mark
    #[arg(long)]
    pub since: Option<String>,
    /// Record the report's snapshot as the last reviewed point
    #[arg(long)]
    pub mark: bool,
    #[arg(long)]
    pub json: bool,
}

pub fn run(ctx: &Ctx, args: SprintArgs) -> Result<()> {
    let report = corral_ops::review_sprint(
        ctx,
        args.since.as_deref().or(args.window.as_deref()),
        args.mark,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.markdown);
        println!("\nPinned report: {}", report.artifact_ref);
    }
    Ok(())
}

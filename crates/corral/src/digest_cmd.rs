use anyhow::Result;
use corral_ops::Ctx;

#[derive(clap::Args)]
pub struct DigestArgs {
    /// Cover the last N hours, days or weeks
    #[arg(long, default_value = "1d")]
    pub since: String,
    #[arg(long)]
    pub json: bool,
}

pub fn run(ctx: &Ctx, args: DigestArgs) -> Result<()> {
    let report = corral_ops::digest(ctx, &args.since)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.markdown);
        println!("\nPinned report: {}", report.artifact_ref);
    }
    Ok(())
}

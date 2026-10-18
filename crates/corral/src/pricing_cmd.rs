use anyhow::{Context, Result};
use clap::Subcommand;
use corral_core::Store;

#[derive(Subcommand)]
pub enum PricingCmd {
    /// Check usage for missing prices, cache-rate fallbacks and catalog overrides
    Check {
        #[arg(long)]
        json: bool,
    },
}

pub fn run(store: &Store, cmd: PricingCmd) -> Result<()> {
    let PricingCmd::Check { json } = cmd;
    let report = corral_ops::pricing_check(store)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("serializing pricing check")?
        );
        return Ok(());
    }
    println!(
        "Catalog: {}; unpriced tokens: {}",
        report.catalog_date, report.unpriced_tokens
    );
    for row in report.unpriced_models {
        println!(
            "unpriced {}: {} tokens ({} entries)",
            row.model.as_deref().unwrap_or("<missing model>"),
            row.tokens,
            row.entries
        );
    }
    for row in report.cache_rate_fallbacks {
        println!(
            "cache fallback {} [{}]: {} entries, {} read / {} write tokens at input rate",
            row.model.as_deref().unwrap_or("<missing model>"),
            row.price_source,
            row.entries,
            row.cache_read_tokens,
            row.cache_write_tokens
        );
    }
    for row in report.shadows {
        println!(
            "shadow {} by config {}: input {} / output {} / cache read {} / cache write {} (catalog {} / {} / {} / {})",
            row.model,
            row.config_model,
            row.config_price.input,
            row.config_price.output,
            row.config_price.cache_read(),
            row.config_price.cache_write(),
            row.catalog_price.input,
            row.catalog_price.output,
            row.catalog_price.cache_read(),
            row.catalog_price.cache_write()
        );
    }
    Ok(())
}

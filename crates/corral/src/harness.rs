use anyhow::Result;
use corral_ops::{Ctx, HarnessInfo, ModelSource};

fn models_cell(h: &HarnessInfo) -> String {
    match h.model_source {
        ModelSource::Listed => format!("{} models", h.models.len()),
        ModelSource::Builtin => format!("{} aliases + free text", h.models.len()),
        ModelSource::FreeText => "free text".into(),
        ModelSource::None => "-".into(),
    }
}

fn effort_cell(h: &HarnessInfo) -> String {
    match (h.takes_effort, h.efforts.is_empty()) {
        (false, _) => "-".into(),
        (true, true) => "any".into(),
        (true, false) => h.efforts.join(","),
    }
}

fn version_cell(h: &HarnessInfo) -> String {
    match (&h.version, h.installed()) {
        (_, false) => "not installed".into(),
        (Some(v), true) => v.clone(),
        (None, true) => "?".into(),
    }
}

pub fn list(ctx: &Ctx, json: bool, refresh: bool) -> Result<()> {
    let report = corral_ops::harness_report(ctx, refresh)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{:<10} {:<14} {:<22} {:<44} PATH",
        "HARNESS", "VERSION", "MODELS", "EFFORT"
    );
    for h in &report.harnesses {
        println!(
            "{:<10} {:<14} {:<22} {:<44} {}",
            h.key,
            version_cell(h),
            models_cell(h),
            effort_cell(h),
            h.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| h.program.clone().unwrap_or_else(|| "-".into()))
        );
        for n in &h.notes {
            println!("           note: {n}");
        }
    }
    println!(
        "detected {}{}",
        report.detected_at,
        if refresh {
            ""
        } else {
            " (cached; --refresh to re-detect)"
        }
    );
    Ok(())
}

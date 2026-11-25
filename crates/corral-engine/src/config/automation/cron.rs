use anyhow::{Context, Result, ensure};
use chrono::DateTime;
use croner::Cron;

/// Match a Unix minute in UTC, accepting only the numeric five-field dialect.
pub fn matches_minute(expression: &str, unix_minute: i64) -> Result<bool> {
    let fields: Vec<_> = expression.split_whitespace().collect();
    ensure!(fields.len() == 5, "expected five UTC cron fields");
    for field in &fields {
        ensure!(
            field
                .bytes()
                .all(|b| b.is_ascii_digit() || b"*,-/".contains(&b)),
            "expected numeric cron fields, lists, ranges or steps"
        );
    }
    let cron: Cron = fields
        .join(" ")
        .parse()
        .context("invalid cron expression")?;
    let seconds = unix_minute.checked_mul(60).context("UTC minute overflow")?;
    let utc = DateTime::from_timestamp(seconds, 0).context("UTC minute outside calendar")?;
    cron.is_time_matching(&utc).context("evaluate UTC cron")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_cron_matches_in_utc_with_either_day_field() -> Result<()> {
        // Unix epoch: a Thursday and the first of the month.
        assert!(matches_minute("0 0 1 * 1-3", 0)?);
        assert!(matches_minute("0 0 2 * 4", 0)?);
        assert!(!matches_minute("0 0 2 * 5", 0)?);
        for sunday in ["0", "7"] {
            assert!(matches_minute(
                &format!("*/15 0,12 * * {sunday}"),
                3 * 1440 + 15
            )?);
        }
        assert!(matches_minute("0 0 ? * *", 0).is_err());
        Ok(())
    }
}

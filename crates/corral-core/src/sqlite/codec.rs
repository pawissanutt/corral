//! Column encodings: fixed-width UTC timestamps (one encoder, so text order is time order),
//! JSON documents, snake_case enum names and checked integer conversions.

use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rusqlite::ErrorCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::backend::{ApplyError, ViewError};

/// `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ` in UTC.
pub(super) fn ts(at: Timestamp) -> String {
    let dt = at.to_zoned(TimeZone::UTC).datetime();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.subsec_nanosecond()
    )
}

pub(super) fn elapsed(earlier: Timestamp, later: Timestamp) -> Duration {
    Duration::try_from(later.duration_since(earlier)).unwrap_or(Duration::ZERO)
}

pub(super) fn add_duration(at: Timestamp, by: Duration) -> Timestamp {
    let step = jiff::SignedDuration::try_from(by).unwrap_or(jiff::SignedDuration::MAX);
    at.saturating_add(step).unwrap_or(at)
}

pub(super) fn opt_ts(at: Option<Timestamp>) -> Option<String> {
    at.map(ts)
}

pub(super) fn parse_ts(text: &str) -> Result<Timestamp> {
    Timestamp::from_str(text).with_context(|| format!("stored timestamp {text:?}"))
}

pub(super) fn parse_opt_ts(text: Option<String>) -> Result<Option<Timestamp>> {
    text.as_deref().map(parse_ts).transpose()
}

pub(super) fn json<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).context("encode document")
}

pub(super) fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_str(text).context("decode stored document")
}

/// The serde name of a unit enum variant (`TaskPart::PlanRun` is `plan_run`).
pub(super) fn name<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value).context("encode name")? {
        Value::String(name) => Ok(name),
        other => bail!("{other} has no stored name"),
    }
}

pub(super) fn from_name<T: DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_value(Value::String(text.to_owned()))
        .with_context(|| format!("stored name {text:?}"))
}

pub(super) fn int(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| anyhow!("{n} exceeds the stored integer range"))
}

pub(super) fn uint(n: i64) -> Result<u64> {
    u64::try_from(n).map_err(|_| anyhow!("stored integer {n} is negative"))
}

pub(super) fn opt_int(n: Option<u64>) -> Result<Option<i64>> {
    n.map(int).transpose()
}

pub(super) fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

impl From<rusqlite::Error> for ApplyError {
    fn from(error: rusqlite::Error) -> Self {
        if is_busy(&error) {
            Self::Busy {
                retry_after: Duration::from_secs(1),
            }
        } else {
            Self::Io(anyhow::Error::new(error).context("sqlite write"))
        }
    }
}

impl From<rusqlite::Error> for ViewError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Io(anyhow::Error::new(error).context("sqlite read"))
    }
}

/// Maps a conversion failure inside an operation to [`ApplyError::Io`].
pub(super) trait OrIo<T> {
    fn or_io(self) -> Result<T, ApplyError>;
}

impl<T> OrIo<T> for Result<T> {
    fn or_io(self) -> Result<T, ApplyError> {
        self.map_err(ApplyError::Io)
    }
}

/// Maps a decoding failure inside a read to [`ViewError::Io`].
pub(super) trait OrView<T> {
    fn or_view(self) -> Result<T, ViewError>;
}

impl<T> OrView<T> for Result<T> {
    fn or_view(self) -> Result<T, ViewError> {
        self.map_err(ViewError::Io)
    }
}

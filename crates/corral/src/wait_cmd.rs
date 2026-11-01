use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{
    Event, EventCursor, EventReader, PM_ACTIONABLE_KINDS, PM_DEFAULT_KINDS, Store, actionable_kind,
    format_event_line,
};

const KINDS: [&str; PM_ACTIONABLE_KINDS.len()] = PM_ACTIONABLE_KINDS;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(clap::Args, Debug)]
pub struct WaitArgs {
    /// Task and configuration event kinds to await (comma-separated)
    #[arg(long = "for", value_delimiter = ',', default_values = PM_DEFAULT_KINDS, value_parser = KINDS)]
    kinds: Vec<String>,
    #[arg(long)]
    task: Option<String>,
    #[arg(long)]
    project: Option<String>,
    /// Maximum wait, e.g. 30m, 2h, 10s (default: unlimited)
    #[arg(long, value_parser = parse_duration)]
    timeout: Option<Duration>,
    #[arg(long)]
    json: bool,
    /// Replay events at or after this RFC3339 timestamp, unless --after is given
    #[arg(long)]
    since: Option<jiff::Timestamp>,
    /// Resume immediately after this event sequence
    #[arg(long)]
    after: Option<u64>,
    #[arg(
        long = "after-offset",
        hide = true,
        num_args = 0..=1,
        default_missing_value = "",
        value_parser = reject_after_offset
    )]
    _after_offset: Option<u64>,
    /// Add a sequence cursor to JSON; append cursor=<JSON> to human output
    #[arg(long)]
    print_cursor: bool,
}

fn reject_after_offset(_: &str) -> Result<u64, String> {
    Err(
        "--after-offset is no longer supported; use --after SEQ (event sequence, not byte offset)"
            .into(),
    )
}

fn parse_duration(text: &str) -> Result<Duration, String> {
    let (number, multiplier) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = text.strip_suffix('h') {
        (n, 3_600_000)
    } else if let Some(n) = text.strip_suffix('d') {
        (n, 86_400_000)
    } else {
        return Err("duration requires ms, s, m, h or d suffix".into());
    };
    let millis = number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or_else(|| format!("invalid duration: {text}"))?;
    Ok(Duration::from_millis(millis))
}

#[derive(Debug)]
pub(crate) struct WaitTimeout;

impl std::fmt::Display for WaitTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("wait timed out")
    }
}

impl std::error::Error for WaitTimeout {}

impl WaitArgs {
    fn matches(&self, store: &Store, event: &Event) -> Result<bool> {
        // Correlation is carried by the producer, so it survives cursor-based rearming.
        // Explicit --for subscriptions can still inspect the mirrored status transition.
        if self
            .kinds
            .iter()
            .map(String::as_str)
            .eq(PM_DEFAULT_KINDS.iter().copied())
            && !corral_core::default_pm_event(event)
        {
            return Ok(false);
        }
        let kind = if matches!(event.kind.as_str(), "config_changed" | "config_invalid") {
            Some(event.kind.as_str())
        } else {
            actionable_kind(event)
        };
        if !kind.is_some_and(|kind| self.kinds.iter().any(|k| k == kind))
            || self.task.as_deref().is_some_and(|id| {
                event.task.as_deref() != Some(id)
                    && !event
                        .detail
                        .get("tasks")
                        .and_then(|tasks| tasks.as_array())
                        .is_some_and(|tasks| tasks.iter().any(|task| task.as_str() == Some(id)))
            })
            || self.since.is_some_and(|since| event.ts < since)
        {
            return Ok(false);
        }
        if let Some(project) = &self.project {
            // Current producers do not include project in the event; resolve its task.
            if let Some(value) = event.detail.get("project").and_then(|p| p.as_str()) {
                return Ok(value == project);
            }
            let Some(id) = &event.task else {
                return Ok(false);
            };
            return Ok(store.head(id)?.project.as_ref() == Some(project));
        }
        Ok(true)
    }
}

fn follow(
    store: &Store,
    args: &WaitArgs,
    reader: &mut EventReader,
    mut idle: impl FnMut(Duration),
) -> Result<Option<(Event, EventCursor)>> {
    let start = Instant::now();
    loop {
        if args.timeout.is_some_and(|limit| start.elapsed() >= limit) {
            return Ok(None);
        }
        let before = reader.read_position();
        if let Some((event, cursor)) = reader.next_event()? {
            if args.matches(store, &event)? {
                return Ok(Some((event, cursor)));
            }
            continue;
        }
        if reader.read_position() != before {
            continue;
        }
        let delay = args.timeout.map_or(POLL_INTERVAL, |limit| {
            POLL_INTERVAL.min(limit.saturating_sub(start.elapsed()))
        });
        idle(delay);
    }
}

fn output(args: &WaitArgs, event: &Event, cursor: &EventCursor) -> Result<String> {
    if args.json {
        let mut value = serde_json::to_value(event).context("serializing wait event")?;
        if args.print_cursor {
            let object = value
                .as_object_mut()
                .context("event must serialize as object")?;
            object.insert("cursor".into(), serde_json::json!({"seq": cursor.seq}));
        }
        return serde_json::to_string(&value).context("serializing wait output");
    }
    let mut line = format_event_line(event);
    if args.print_cursor {
        line.push_str(&format!(
            " cursor={}",
            serde_json::json!({"seq": cursor.seq})
        ));
    }
    Ok(line)
}

pub(crate) fn run(store: &Store, mut args: WaitArgs) -> Result<()> {
    args.task = args.task.as_deref().map(Store::normalize_id).transpose()?;
    if args.kinds.is_empty() {
        bail!("at least one --for kind is required");
    }
    let mut reader = if let Some(seq) = args.after {
        store.events_since(Some(EventCursor {
            seq,
            ..EventCursor::default()
        }))?
    } else if args.since.is_some() {
        store.events_since(Some(EventCursor::default()))?
    } else {
        store.events_since(None)?
    };
    let Some((event, cursor)) = follow(store, &args, &mut reader, std::thread::park_timeout)?
    else {
        return Err(WaitTimeout.into());
    };
    println!("{}", output(&args, &event, &cursor)?);
    Ok(())
}

#[cfg(test)]
#[path = "wait_cmd_tests.rs"]
mod tests;

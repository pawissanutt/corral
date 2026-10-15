//! The live preview of the Board cards page: a sample task drawn by the real card renderer from the staged config.

use corral_core::{Config, Task};
use jiff::Timestamp;
use serde_json::json;

use super::model::{Field, Kind, Row};
use crate::board::{CardExtra, card_lines};

const WIDE_CARD_W: usize = 22;
const NARROW_CARD_W: usize = 9;

fn sample_task() -> Option<Task> {
    let step = |id: &str, state: &str| json!({"id": id, "label": id, "status": {"state": state}});
    serde_json::from_value(json!({
        "id": "T-071",
        "title": "Sample task",
        "status": "running",
        "priority": "P1",
        "depends_on": ["T-050"],
        "created": "2026-01-01T11:00:00Z",
        "updated": "2026-01-01T11:55:00Z",
        "runs": [{
            "id": "R1", "agent": "sonnet", "harness": "claude", "model": "claude-sonnet-4", "effort": "high",
            "worktree": "/sample", "branch": "corral/T-071", "base": "main", "base_sha": "0",
            "started": "2026-01-01T11:48:00Z",
            "usage": {"input": 48200, "output": 0, "cache_read": 0, "cache_write": 0,
                      "reported_usd": 1.42, "cost_usd": 1.42},
            "progress": {"revision": 1, "spec_revision": "sample", "phases": [{
                "id": "build", "label": "Build",
                "steps": [step("a", "done"), step("b", "done"), step("c", "in_progress"), step("d", "pending")]
            }]},
            "autopilot": {"stage": "reviewing", "at": "2026-01-01T11:59:00Z"}
        }],
        "artifacts": [{
            "run": "R1", "kind": "diff", "title": "Final diff +120 −14", "path": "a", "source": "/a",
            "sha256": "0", "bytes": 1, "added": "2026-01-01T11:58:00Z"
        }]
    }))
    .ok()
}

fn card_rows(task: &Task, cfg: &Config, width: usize, label: &str, first: usize) -> Vec<Row> {
    let now: Timestamp = "2026-01-01T12:00:00Z"
        .parse()
        .unwrap_or(Timestamp::UNIX_EPOCH);
    card_lines(task, &[], cfg, width, false, now, CardExtra::default())
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let text: String = line
                .spans
                .iter()
                .skip(1)
                .map(|s| s.content.as_ref())
                .collect();
            let label = if i == 0 { label } else { "" };
            Row::new(Field::CardPreview(first + i), label, Kind::Info, text)
        })
        .collect()
}

/// The sample card at the 160x45 and 80x24 card widths, so a field the narrow board would drop is not promised.
pub(super) fn rows(cfg: &Config) -> Vec<Row> {
    let Some(task) = sample_task() else {
        return Vec::new();
    };
    let mut out = card_rows(&task, cfg, WIDE_CARD_W, "At 160x45", 0);
    out.extend(card_rows(&task, cfg, NARROW_CARD_W, "At 80x24", 100));
    out
}

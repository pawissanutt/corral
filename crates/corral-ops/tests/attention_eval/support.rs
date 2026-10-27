//! Shared scripted-PM and measurement helpers. Every metric is read through `metrics::stats`.
use corral_ops::attention::metrics::{self, Stats, Window};
use jiff::Timestamp;

use crate::fixture::{Fx, at};

/// Fixture clocks start in 2099 while appended events carry the real wall clock, so windows
/// open early enough to include those appends without reading the clock.
pub fn window(end: i64) -> Window {
    Window {
        start: "2000-01-01T00:00:00Z".parse().unwrap(),
        end: at(end),
    }
}

pub fn stats(fx: &Fx, end: i64) -> Stats {
    metrics::stats(&fx.reopen(), window(end)).unwrap()
}

/// A live PM pane: the fake engine reports any pane as present and working.
pub fn pm_live(fx: &Fx) {
    std::fs::write(
        fx.ctx.store.state_dir().join("pm.json"),
        r#"{"pane_id":"pm:p1"}"#,
    )
    .unwrap();
}

pub fn pm_gone(fx: &Fx) {
    std::fs::remove_file(fx.ctx.store.state_dir().join("pm.json")).unwrap();
}

pub fn sync(fx: &Fx, seconds: i64) {
    corral_ops::sync_at(&fx.reopen(), at(seconds)).unwrap();
}

pub fn keys(fx: &Fx, id: &str) -> Vec<String> {
    fx.ctx
        .store
        .get(id)
        .unwrap()
        .attention
        .records
        .keys()
        .map(|key| key.0.clone())
        .collect()
}

pub fn only_key(fx: &Fx, id: &str) -> String {
    let keys = keys(fx, id);
    assert_eq!(keys.len(), 1, "{keys:?}");
    keys[0].clone()
}

/// A scripted PM or owner operation, run in a fresh process with a scrubbed agent environment
/// because `attend`, `park` and `unpark` refuse worker contexts exactly as the real CLI does.
pub fn op(root: &std::path::Path, pm: bool, vars: &[(&str, String)]) -> String {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "attend_subprocess", "--nocapture"])
        .env("EVAL_ROOT", root);
    for name in [
        "AI_AGENT",
        "PI_CODING_AGENT",
        "CORRAL_REVIEWER",
        "CORRAL_AGENT",
        "CORRAL_AGENT_TASK",
        "CORRAL_PM",
    ] {
        command.env_remove(name);
    }
    if pm {
        command.env("CORRAL_PM", "1");
    }
    for (name, value) in vars {
        command.env(name, value);
    }
    let output = command.stdin(std::process::Stdio::null()).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

pub fn attend(fx: &Fx, key: &str, request: &str, next_check: u64, seconds: i64) -> String {
    op(
        fx.ctx.store.root(),
        true,
        &[
            ("EVAL_OP", "attend".into()),
            ("EVAL_KEY", key.into()),
            ("EVAL_REQUEST", request.into()),
            ("EVAL_NEXT", next_check.to_string()),
            ("EVAL_AT", seconds.to_string()),
        ],
    )
}

pub fn park(root: &std::path::Path, pm: bool, id: &str, checks: &str, seconds: i64) {
    op(
        root,
        pm,
        &[
            ("EVAL_OP", "park".into()),
            ("EVAL_KEY", id.into()),
            ("EVAL_CHECKS", checks.into()),
            ("EVAL_AT", seconds.to_string()),
        ],
    );
}

pub fn unpark(root: &std::path::Path, pm: bool, id: &str, revision: u64) {
    op(
        root,
        pm,
        &[
            ("EVAL_OP", "unpark".into()),
            ("EVAL_KEY", id.into()),
            ("EVAL_NEXT", revision.to_string()),
        ],
    );
}

pub fn attend_child() {
    let Ok(root) = std::env::var("EVAL_ROOT") else {
        return;
    };
    let var = |name: &str| std::env::var(name).unwrap();
    let ctx =
        crate::Ctx::from_store(corral_core::Store::open(std::path::Path::new(&root)).unwrap());
    match var("EVAL_OP").as_str() {
        "attend" => {
            let handling = corral_ops::attention::attend(
                &ctx,
                &var("EVAL_KEY"),
                "Investigating",
                var("EVAL_NEXT").parse().unwrap(),
                &var("EVAL_REQUEST"),
                at(var("EVAL_AT").parse().unwrap()),
            )
            .unwrap();
            println!("EVAL_REVISION={}", handling.revision);
        }
        "park" => {
            let checks: Vec<_> = var("EVAL_CHECKS")
                .split(',')
                .filter(|check| !check.is_empty())
                .map(|check| corral_core::parse_park_check(check).unwrap())
                .collect();
            corral_ops::park::park(
                &ctx,
                &var("EVAL_KEY"),
                "Wait for prerequisites",
                &checks,
                "Inspect retained work before resuming",
                86400,
                at(var("EVAL_AT").parse().unwrap()),
            )
            .unwrap();
        }
        "unpark" => {
            corral_ops::park::unpark(&ctx, &var("EVAL_KEY"), var("EVAL_NEXT").parse().unwrap())
                .unwrap();
        }
        other => panic!("unknown op {other}"),
    }
}

/// Zero unfinished tasks without an authorized executor's progress or a derived condition.
pub fn covered(ctx: &crate::Ctx, seconds: i64) {
    let board = corral_ops::attention::board(ctx, at(seconds)).unwrap();
    for row in &board.tasks {
        if matches!(
            row.status,
            corral_core::Status::Done | corral_core::Status::Dropped
        ) {
            continue;
        }
        assert!(
            row.assessment.progress.is_some() || !row.assessment.conditions.is_empty(),
            "{} has neither progress nor conditions",
            row.task
        );
    }
}

pub fn record(fx: &Fx, id: &str, key: &str) -> corral_core::AttentionRecord {
    fx.ctx
        .store
        .get(id)
        .unwrap()
        .attention
        .records
        .get(&corral_core::attention::ConditionKey(key.into()))
        .cloned()
        .unwrap_or_else(|| panic!("no record {key}"))
}

pub fn ts(seconds: i64) -> Option<Timestamp> {
    Some(at(seconds))
}

/// Offline thresholds that hold for every scenario's final state.
pub fn invariants(stats: &Stats) {
    assert_eq!(stats.actions.duplicates, 0, "duplicate actions");
    assert!(stats.wakes.max_levels_per_condition <= 2, "attention wakes");
    assert!(
        stats.wakes.attention_due <= stats.wakes.due_pairs,
        "attention_due per (project, pass)"
    );
    assert!(stats.notifications.calls[0] <= stats.notifications.batches);
}

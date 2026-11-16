use super::*;
use corral_core::{ProgressPhase, ProgressStep};

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

#[test]
fn activity_buckets_keep_minute_boundaries_and_twenty_minute_limit() {
    let now = at("2026-11-16T12:20:45Z");
    let mut records = Records::new(now);
    for timestamp in [
        "2026-11-16T12:00:59.999Z", // Outside the twenty buckets.
        "2026-11-16T12:01:00Z",
        "2026-11-16T12:19:59.999Z",
        "2026-11-16T12:20:00Z",
        "2026-11-16T12:20:44Z",
        "2026-11-16T12:21:00Z", // Not counted before its minute.
    ] {
        records.observe(at(timestamp));
    }
    let output = records.finish(Vec::new());
    let activity = output.activity.unwrap();
    assert_eq!(activity.until, at("2026-11-16T12:20:00Z").to_string());
    assert_eq!(activity.per_minute.len(), AGENT_ACTIVITY_MINUTES);
    assert_eq!(activity.per_minute[0], 1);
    assert_eq!(activity.per_minute[18], 1);
    assert_eq!(activity.per_minute[19], 2);
    assert_eq!(activity.per_minute.iter().sum::<u16>(), 4);

    let mut saturated = Records::new(now);
    for _ in 0..=u16::MAX {
        saturated.observe(now);
    }
    assert_eq!(
        saturated.finish(Vec::new()).activity.unwrap().per_minute[19],
        u16::MAX
    );
}

#[test]
fn activity_future_record_cannot_mask_valid_last_output() {
    let now = at("2026-11-16T12:20:45Z");
    let mut records = Records::new(now);
    records.observe(at("2026-11-16T12:20:30Z"));
    records.observe(at("2026-11-16T12:20:45.000000001Z"));
    records.observe(at("2026-11-16T12:21:00Z"));
    records.observe(now);
    let output = records.finish(Vec::new());
    assert_eq!(output.last_output_at, Some(now.to_string()));
    let activity = output.activity.unwrap();
    assert_eq!(activity.per_minute[19], 2);
    assert_eq!(activity.per_minute.iter().sum::<u16>(), 2);
}

#[test]
fn activity_idle_row_and_revision_do_not_tick_with_the_clock() {
    let row = |now| {
        let mut records = Records::new(now);
        records.observe(at("2026-11-16T11:59:59Z"));
        let mut row = super::super::blank("pm".into(), "pm", "");
        records.finish(Vec::new()).apply(&mut row);
        row.rev = super::super::revision(&row).unwrap();
        row
    };
    let first = row(at("2026-11-16T12:20:00Z"));
    let later = row(at("2026-11-16T14:00:00Z"));
    assert!(first.activity.is_none());
    assert!(first.last_output_at.is_some());
    assert_eq!(first, later);

    let active = row(at("2026-11-16T12:00:00Z"));
    assert!(active.activity.is_some());
    assert_eq!(active, row(at("2026-11-16T12:00:59.999Z")));
}

#[test]
fn activity_steps_flatten_phases_in_order_and_cap_the_wire_list() {
    let states = [
        StepState::Pending,
        StepState::InProgress,
        StepState::Done,
        StepState::Dropped {
            reason: "superseded".into(),
        },
    ];
    let phase = |start: usize, count| ProgressPhase {
        id: start.to_string(),
        label: String::new(),
        steps: (start..start + count)
            .map(|index| ProgressStep {
                id: index.to_string(),
                label: format!("{index}:{}", "é".repeat(90)),
                status: states[index % states.len()].clone(),
                criteria: Vec::new(),
            })
            .collect(),
    };
    let progress = RunProgress {
        revision: 1,
        spec_revision: String::new(),
        phases: vec![phase(0, 3), phase(3, AGENT_STEPS_MAX)],
        plan: None,
        waits: Vec::new(),
    };
    let steps = steps(Some(&progress)).unwrap();
    assert_eq!(steps.len(), AGENT_STEPS_MAX);
    let mapped = [
        AgentStepState::Pending,
        AgentStepState::InProgress,
        AgentStepState::Done,
        AgentStepState::Dropped,
    ];
    for (index, step) in steps.iter().enumerate() {
        assert!(step.title.starts_with(&format!("{index}:")));
        assert!(step.title.len() <= 120);
        assert_eq!(step.state, mapped[index % mapped.len()]);
    }
    assert!(super::steps(None).is_none());
}

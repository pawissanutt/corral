//! Three synthetic build records through the live Builds path: one landed after two review
//! rounds, one landed over an open finding by the owner's override, one still in flight. The
//! owner's words are marked as preview; nothing here is a task.

use super::*;

const MINUTE: u64 = 60;

fn at(feed: &Feed, minutes_ago: u64) -> String {
    stamp(feed, feed.now.saturating_sub(minutes_ago * MINUTE))
}

fn usage(input: u64, output: u64, cost_usd: Option<f64>) -> BuildUsage {
    BuildUsage {
        input,
        output,
        cost_usd,
    }
}

fn slice(
    feed: &Feed,
    (id, title, kind, model): (&str, &str, &str, &str),
    state: &str,
    (started, ended): (u64, Option<u64>),
    (tokens, cost_usd, turns): (u64, Option<f64>, u64),
) -> BuildSlice {
    BuildSlice {
        id: id.into(),
        title: title.into(),
        kind: kind.into(),
        model: model.into(),
        effort: Some("high".into()),
        state: state.into(),
        started: Some(at(feed, started)),
        ended: ended.map(|minutes| at(feed, minutes)),
        tokens,
        cost_usd,
        turns,
        outside: vec![],
        report: None,
    }
}

fn gate(
    feed: &Feed,
    attempt: &str,
    head: &str,
    result: &str,
    (from, to): (u64, u64),
) -> BuildVerification {
    BuildVerification {
        attempt: attempt.into(),
        tier: "full".into(),
        head: head.into(),
        result: result.into(),
        failed_command: (result == "fail").then(|| "cargo test -p corral-core".into()),
        started: at(feed, from),
        ended: at(feed, to),
    }
}

fn finding(
    severity: &str,
    (path, line): (&str, u64),
    text: &str,
    resolution: Option<BuildResolution>,
) -> BuildFinding {
    BuildFinding {
        id: format!("{path}:{line}"),
        severity: severity.into(),
        path: Some(path.into()),
        line: Some(line),
        text: text.into(),
        resolution,
    }
}

fn fixed(commit: &str) -> Option<BuildResolution> {
    Some(BuildResolution {
        state: "fixed".into(),
        commit: Some(commit.into()),
        reason: None,
    })
}

fn timeline(feed: &Feed, phases: &[(u64, &str, &str)]) -> Vec<BuildTimeline> {
    phases
        .iter()
        .map(|&(minutes, phase, detail)| BuildTimeline {
            at: at(feed, minutes),
            phase: phase.into(),
            detail: (!detail.is_empty()).then(|| detail.into()),
        })
        .collect()
}

/// The detail with every list empty; each record fills what it has.
fn record(id: &str, title: &str, outcome: BuildOutcome, total: BuildUsage) -> BuildDetail {
    BuildDetail {
        id: id.into(),
        kind: "build".into(),
        project: Some("corral".into()),
        title: title.into(),
        rev: 1,
        state: outcome.state.clone(),
        outcome,
        branch: Some(format!("build/{}", id.trim_start_matches("B-"))),
        base: Some("main".into()),
        intent: vec![],
        intent_total: 0,
        diff: None,
        commits: vec![],
        commits_total: 0,
        slices: vec![],
        slices_total: 0,
        verification: vec![],
        verification_total: 0,
        reviews: vec![],
        reviews_total: 0,
        messages: vec![],
        messages_total: 0,
        decisions: vec![],
        decisions_total: 0,
        usage: BuildUsageTotals {
            parent: usage(
                total.input / 3,
                total.output / 3,
                total.cost_usd.map(|c| c / 3.0),
            ),
            children: usage(
                total.input / 2,
                total.output / 2,
                total.cost_usd.map(|c| c / 2.0),
            ),
            reviewers: usage(
                total.input / 6,
                total.output / 6,
                total.cost_usd.map(|c| c / 6.0),
            ),
            total,
        },
        timeline: vec![],
        timeline_total: 0,
    }
}

/// Lists the record as corrald's row does and serves its detail, totals counted from it.
fn publish(feed: &mut Feed, detail: BuildDetail, updated: u64) {
    let gate_failures = detail
        .verification
        .iter()
        .filter(|gate| gate.result == "fail")
        .count() as u64;
    let row = BuildRow {
        id: detail.id.clone(),
        kind: detail.kind.clone(),
        project: detail.project.clone(),
        title: detail.title.clone(),
        state: detail.state.clone(),
        created: detail
            .intent
            .first()
            .map_or_else(|| at(feed, updated), |said| said.at.clone()),
        updated: at(feed, updated),
        landed: detail.outcome.landed.clone(),
        slices: detail.slices_total,
        review_rounds: detail.reviews_total,
        gate_attempts: detail.verification_total,
        gate_failures,
        messages: detail.messages_total,
        cost_usd: detail.usage.total.cost_usd,
        tokens: detail.usage.total.input + detail.usage.total.output,
        override_reason: detail.outcome.override_reason.clone(),
        rev: detail.rev,
    };
    let id = detail.id.clone();
    feed.builds.insert(id.as_str().into(), row);
    put(feed, DetailRef::Build { id }, DetailBody::Build(detail));
}

pub(super) fn builds(feed: &mut Feed) {
    landed(feed);
    overridden(feed);
    in_flight(feed);
}

fn landed(feed: &mut Feed) {
    let mut build = record(
        "B-record-store",
        "Build records: bounded store and app read",
        BuildOutcome {
            state: "landed".into(),
            at: Some(at(feed, 95)),
            head: Some("7c41d9e02b".into()),
            landed: Some("e3a0b77f10".into()),
            base_branch: Some("main".into()),
            override_reason: None,
        },
        usage(2_480_000, 312_000, Some(14.6)),
    );
    build.intent = vec![BuildIntent {
        at: at(feed, 260),
        text: "Preview · Store build records next to the task store, bounded, and serve them to \
               the app with totals so truncation is visible."
            .into(),
    }];
    build.intent_total = 1;
    build.diff = Some(BuildDiff {
        commits: 4,
        files: 11,
        added: 642,
        removed: 58,
    });
    build.commits = [
        (
            "51d2e8a4c1",
            "store: bounded build record files",
            Some("S1"),
        ),
        (
            "a90c3b1e77",
            "ops: app read for build rows and detail",
            Some("S2"),
        ),
        (
            "2f6e0d9b35",
            "ops: keep a contiguous recent suffix under the byte budget",
            None,
        ),
        (
            "7c41d9e02b",
            "store: refuse records over the commit cap",
            None,
        ),
    ]
    .into_iter()
    .map(|(sha, subject, slice)| BuildCommit {
        sha: sha.into(),
        subject: subject.into(),
        slice: slice.map(str::to_owned),
    })
    .collect();
    build.commits_total = 4;
    let mut store = slice(
        feed,
        ("S1", "Record store", "slice", "gpt-6.1-sol"),
        "done",
        (240, Some(190)),
        (910_000, Some(5.2), 38),
    );
    store.report = Some(
        "Preview · Records live under the store's builds directory, one file each, \
        validated on write. Listing reads summaries only."
            .into(),
    );
    let mut read = slice(
        feed,
        ("S2", "App read", "slice", "gpt-6.1-sol"),
        "done",
        (235, Some(180)),
        (740_000, Some(4.1), 31),
    );
    read.outside = vec!["crates/corral-ops/src/app_read/paging.rs".into()];
    build.slices = vec![
        store,
        read,
        slice(
            feed,
            (
                "E1",
                "Where detail paging is bounded",
                "explore",
                "gpt-6-luna",
            ),
            "done",
            (250, Some(244)),
            (120_000, Some(0.3), 9),
        ),
    ];
    build.slices_total = 3;
    build.verification = vec![
        gate(feed, "g1", "2f6e0d9b35", "fail", (170, 160)),
        gate(feed, "g2", "7c41d9e02b", "pass", (130, 118)),
    ];
    build.verification_total = 2;
    build.reviews = vec![
        BuildReview {
            round: 1,
            reviewer: "review-1".into(),
            model: "claude-opus-4-5".into(),
            head: "2f6e0d9b35".into(),
            verdict: "changes".into(),
            at: at(feed, 150),
            summary: Some("Preview · The byte budget can split a message list mid-entry.".into()),
            findings: vec![
                finding(
                    "major",
                    ("crates/corral-ops/src/app_read/builds.rs", 212),
                    "A partial message can be presented when the budget runs out.",
                    fixed("7c41d9e02b"),
                ),
                finding(
                    "minor",
                    ("crates/corral-core/src/store/build_records.rs", 88),
                    "Commit cap is not enforced on write.",
                    fixed("7c41d9e02b"),
                ),
            ],
        },
        BuildReview {
            round: 2,
            reviewer: "review-2".into(),
            model: "claude-opus-4-5".into(),
            head: "7c41d9e02b".into(),
            verdict: "approve".into(),
            at: at(feed, 110),
            summary: Some("Preview · Both findings are fixed; no new defect.".into()),
            findings: vec![],
        },
    ];
    build.reviews_total = 2;
    // corrald sends the newest 100 of a longer conversation.
    build.messages = (0..100u64)
        .map(|n| BuildMessage {
            at: at(feed, 230 - n),
            from: if n % 2 == 0 { "parent" } else { "S1" }.into(),
            to: if n % 2 == 0 { "S1" } else { "parent" }.into(),
            text: if n % 2 == 0 {
                format!("Preview · Status of step {}?", n / 2 + 1)
            } else {
                format!("Preview · Step {} done; tests pass locally.", n / 2 + 1)
            },
            delivery: "delivered".into(),
        })
        .collect();
    build.messages_total = 140;
    build.decisions = vec![BuildDecision {
        at: at(feed, 100),
        kind: "exit".into(),
        text: "Land on main?".into(),
        answer: Some("Preview · Yes, land it.".into()),
    }];
    build.decisions_total = 1;
    build.timeline = timeline(
        feed,
        &[
            (260, "started", ""),
            (250, "children", "3 child agents"),
            (170, "verify", "gate failed: cargo test"),
            (150, "review", "round 1: changes requested"),
            (130, "verify", "gate passed"),
            (110, "review", "round 2: approved"),
            (95, "land", "e3a0b77 on main"),
            (94, "exit", ""),
        ],
    );
    build.timeline_total = 8;
    publish(feed, build, 94);
}

fn overridden(feed: &mut Feed) {
    let mut build = record(
        "B-peer-roster",
        "Peer roster across sessions",
        BuildOutcome {
            state: "landed".into(),
            at: Some(at(feed, 30)),
            head: Some("c0ffee4d21".into()),
            landed: Some("9b8d6a1c55".into()),
            base_branch: Some("main".into()),
            override_reason: Some(
                "Preview · The finding is in generated schema output; tracked separately.".into(),
            ),
        },
        // One child ran on a model with no price, so the total cost is unknown.
        usage(1_150_000, 141_000, None),
    );
    build.intent = vec![BuildIntent {
        at: at(feed, 120),
        text: "Preview · Let agents in other sessions see each other in the roster.".into(),
    }];
    build.intent_total = 1;
    build.slices = vec![slice(
        feed,
        ("S1", "Roster transport", "slice", "local-qwen"),
        "done",
        (115, Some(70)),
        (600_000, None, 24),
    )];
    build.slices_total = 1;
    build.verification = vec![gate(feed, "g1", "c0ffee4d21", "pass", (60, 52))];
    build.verification_total = 1;
    build.reviews = vec![BuildReview {
        round: 1,
        reviewer: "review-1".into(),
        model: "claude-opus-4-5".into(),
        head: "c0ffee4d21".into(),
        verdict: "changes".into(),
        at: at(feed, 45),
        summary: None,
        findings: vec![finding(
            "major",
            ("crates/corral-protocol/schema/corral.v1.json", 4012),
            "Schema lists peer_roster twice.",
            Some(BuildResolution {
                state: "open".into(),
                commit: None,
                reason: None,
            }),
        )],
    }];
    build.reviews_total = 1;
    build.decisions = vec![BuildDecision {
        at: at(feed, 32),
        kind: "override".into(),
        text: "Land over the open review finding?".into(),
        answer: Some("Preview · Yes; the schema fix is separate work.".into()),
    }];
    build.decisions_total = 1;
    build.timeline = timeline(
        feed,
        &[
            (120, "started", ""),
            (60, "verify", "gate passed"),
            (45, "review", "round 1: changes requested"),
            (30, "land", "landed by override"),
        ],
    );
    build.timeline_total = 4;
    publish(feed, build, 29);
}

fn in_flight(feed: &mut Feed) {
    let mut build = record(
        "B-fleet-wait",
        "Non-blocking child agents with agent_wait",
        BuildOutcome {
            state: "open".into(),
            at: None,
            head: None,
            landed: None,
            base_branch: None,
            override_reason: None,
        },
        usage(420_000, 51_000, Some(3.4)),
    );
    build.intent = vec![BuildIntent {
        at: at(feed, 40),
        text: "Preview · Children should not block the parent; deliver their results as \
               messages."
            .into(),
    }];
    build.intent_total = 1;
    build.slices = vec![
        slice(
            feed,
            ("S1", "Runtime settlement", "slice", "gpt-6.1-sol"),
            "running",
            (35, None),
            (260_000, Some(2.1), 12),
        ),
        slice(
            feed,
            ("R1", "Review of the wait tool", "review", "claude-opus-4-5"),
            "queued",
            (5, None),
            (0, None, 0),
        ),
    ];
    build.slices_total = 2;
    build.messages = vec![BuildMessage {
        at: at(feed, 8),
        from: "S1".into(),
        to: "parent".into(),
        text: "Preview · Is cancellation in scope for this slice?".into(),
        delivery: "queued".into(),
    }];
    build.messages_total = 1;
    build.timeline = timeline(
        feed,
        &[(40, "started", ""), (36, "children", "2 child agents")],
    );
    build.timeline_total = 2;
    publish(feed, build, 4);
}

//! The sample build's parts on the sample's clock, as `packages/corral-pi/test/fixtures/sessions/
//! parts.json` tells them: its three children with their details and conversations, the messages
//! between them, two jobs, three changed files with their patches, usage, context, the plan, the
//! board with its proposal, the build record and the host's events.

use corral_protocol::detail::*;
use corral_protocol::session_state::InputOrigin;
use corral_protocol::subscription::SessionAgentMessage;

use super::parts::{Child, Job, Parts};
use super::parts_sample::{
    OPUS, OPUS_API, PARENT, Place, command, fresh, proposal, totals, usage_row,
};
use super::pi::tool_item;
use super::sample::{MINUTE, sha};
use super::stamp;
use crate::model::daemon::Feed;

fn said(at: &str, author: &str, model: Option<&str>, items: Vec<SessionItem>) -> SessionMessage {
    SessionMessage {
        id: String::new(),
        author: author.into(),
        at: Some(at.into()),
        items,
        model: model.map(str::to_owned),
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    }
}

/// A user message of a child's transcript: its brief from the parent, or the owner's steer.
fn asked(at: &str, text: &str, origin: InputOrigin) -> SessionMessage {
    SessionMessage {
        input: Some(InputFacts {
            id: None,
            origin,
            typed_at: None,
            device: None,
            to: None,
        }),
        ..said(
            at,
            "user",
            None,
            vec![SessionItem::Text { text: text.into() }],
        )
    }
}

pub(super) fn text(words: &str) -> SessionItem {
    SessionItem::Text { text: words.into() }
}

pub(super) fn tool(call: &str, name: &str, args: &str, lines: &[&str]) -> SessionItem {
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    tool_item(call.into(), name, args, &lines, true)
}

fn steer(at: String, words: &str) -> ChildSteer {
    ChildSteer {
        at: Some(at),
        text: words.into(),
    }
}

fn profile(name: &str, model: &str, effort: &str, description: &str) -> ChildProfile {
    ChildProfile {
        name: name.into(),
        model: model.into(),
        effort: Some(effort.into()),
        description: Some(description.into()),
    }
}

fn file(
    path: &str,
    from: Option<&str>,
    status: &str,
    (added, removed): (u32, u32),
    author: Option<&str>,
    outside: bool,
) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        original_path: from.map(str::to_owned),
        status: status.into(),
        added: Some(added),
        removed: Some(removed),
        authors: author.map(str::to_owned).into_iter().collect(),
        outside,
    }
}

fn patch(path: &str, patch: &str, truncated: bool) -> PatchPage {
    PatchPage {
        path: path.into(),
        patch: patch.into(),
        truncated,
        candidate: None,
    }
}

fn step(id: &str, words: &str, state: &str, detail: Option<&str>, wait: Option<&str>) -> PlanStep {
    PlanStep {
        id: id.into(),
        text: words.into(),
        state: state.into(),
        detail: detail.map(str::to_owned),
        wait: wait.map(str::to_owned),
    }
}

fn task(
    id: &str,
    title: &str,
    (status, phase): (&str, &str),
    depends: &[&str],
    detail: Option<&str>,
    at: String,
) -> BoardTask {
    BoardTask {
        id: id.into(),
        key: None,
        project: None,
        title: title.into(),
        status: status.into(),
        phase: phase.into(),
        size: None,
        difficulty: None,
        tags: Vec::new(),
        depends: depends.iter().map(|id| (*id).to_owned()).collect(),
        detail: detail.map(str::to_owned),
        updated_at: at,
    }
}

/// The sample build's parts on the sample's clock.
pub(super) fn sample(feed: &Feed, id: &str, place: &Place) -> Parts {
    let now = feed.now;
    let at = |minutes: u64| stamp(now.saturating_sub(minutes * MINUTE));
    let ms = |minutes: u64| now.saturating_sub(minutes * MINUTE).saturating_mul(1000);
    let tree = place.worktree.as_str();
    let (base, first, second) = (
        sha("parity/base", 40),
        sha("parity/docs", 40),
        sha("parity/migration", 40),
    );
    let mut parts = fresh(feed, id);
    parts.children = children(&at, &ms, &first);
    parts.messages = vec![
        SessionAgentMessage {
            seq: 1,
            at: ms(26),
            from: "parent".into(),
            to: "migration".into(),
            text: "Keep the compatibility note in the first section.".into(),
            delivery: "delivered".into(),
        },
        SessionAgentMessage {
            seq: 2,
            at: ms(24),
            from: "migration".into(),
            to: "parent".into(),
            text: "The note is ready; it touches a path outside my brief.".into(),
            delivery: "delivered".into(),
        },
        SessionAgentMessage {
            seq: 3,
            at: ms(23),
            from: "review".into(),
            to: "parent".into(),
            text: "Review complete. Add a compatibility note before publishing.".into(),
            delivery: "delivered".into(),
        },
    ];
    parts.jobs = vec![
        Job {
            job: SessionJob {
                id: "j1-1".into(),
                command: "watch parity snapshot".into(),
                dir: Some(tree.into()),
                state: "running".into(),
                started_at: Some(at(26)),
                ended_at: None,
                exit_code: None,
                output_bytes: 0,
                tail: vec!["Snapshot 1 ready.".into(), "Waiting for changes.".into()],
                monitors: vec!["Snapshot".into()],
                actions: vec!["stop".into(), "background".into()],
            },
            older: "Starting the snapshot watch.\n".into(),
            newest: "Snapshot 1 ready.\nWaiting for changes.\n".into(),
        },
        Job {
            job: SessionJob {
                id: "j1-2".into(),
                command: "test -f docs/compatibility.md".into(),
                dir: Some(tree.into()),
                state: "exited".into(),
                started_at: Some(at(27)),
                ended_at: Some(at(27)),
                exit_code: Some(1),
                output_bytes: 0,
                tail: vec!["Check failed: missing note.".into()],
                monitors: Vec::new(),
                actions: Vec::new(),
            },
            older: String::new(),
            newest: "Check failed: missing note.\n".into(),
        },
    ];
    for job in &mut parts.jobs {
        job.job.output_bytes = (job.older.len() + job.newest.len()) as u64;
    }
    parts.changes.comparisons.push(ComparisonInfo {
        id: ChangesComparison::Candidate,
        label: "Task candidate".into(),
        unavailable: Some("This build session has no bound task candidate".into()),
    });
    parts.changes.scopes = vec![ChangeScope {
        id: "bound".into(),
        label: "Session worktree".into(),
    }];
    parts.changes.base = Some(base);
    parts.changes.head = Some(second.clone());
    parts.changes.files = vec![
        file("line.txt", None, "M", (2, 2), None, false),
        file(
            "docs/parity-guide.md",
            Some("docs/draft-guide.md"),
            "R",
            (1, 1),
            Some("docs"),
            false,
        ),
        file(
            "docs/compatibility.md",
            None,
            "A",
            (20, 0),
            Some("migration"),
            true,
        ),
    ];
    parts.changes.total = 3;
    parts.changes.added = 23;
    parts.changes.removed = 3;
    parts.patches = vec![
        patch(
            "line.txt",
            "diff --git a/line.txt b/line.txt\n--- a/line.txt\n+++ b/line.txt\n@@ -1,2 +1,2 @@\n-original\n+parity\n   first example\n@@ -11,2 +11,2 @@\n   final example\n-original label\n+parity label\n",
            false,
        ),
        patch(
            "docs/parity-guide.md",
            "diff --git a/docs/draft-guide.md b/docs/parity-guide.md\nsimilarity index 95%\nrename from docs/draft-guide.md\nrename to docs/parity-guide.md\n--- a/docs/draft-guide.md\n+++ b/docs/parity-guide.md\n@@ -1 +1 @@\n-# Draft guide\n+# Parity guide\n",
            false,
        ),
        patch(
            "docs/compatibility.md",
            "diff --git a/docs/compatibility.md b/docs/compatibility.md\nnew file mode 100644\n--- /dev/null\n+++ b/docs/compatibility.md\n@@ -0,0 +1,20 @@\n+# Compatibility\n+\n+Use the parity label in new projects.\n",
            true,
        ),
    ];
    parts.pane = UsageRow {
        context_percent: Some(14.0),
        ..usage_row("pane", "Pane", "session", 11_100, 900, Some(0.09))
    };
    parts.helpers = vec![usage_row(
        "child:oracle-guide",
        "oracle",
        "helper:oracle",
        0,
        0,
        None,
    )];
    parts.context_tokens = 28_000;
    parts
        .context
        .context_files
        .push(format!("{tree}/docs/CONTRIBUTING.md"));
    parts.context.skills = vec!["migration-guide".into(), "review-notes".into()];
    parts.context.prompts = vec!["parity-summary".into()];
    parts.context.mcp = vec![
        McpServer {
            name: "project-index".into(),
            state: "connected".into(),
        },
        McpServer {
            name: "docs-index".into(),
            state: "unavailable".into(),
        },
    ];
    parts.plan = SessionPlan {
        plan: Some(Plan {
            title: "Guide and migration".into(),
            revision: 12,
            steps: vec![
                step(
                    "baseline",
                    "Capture the starting label",
                    "done",
                    Some("Original label recorded"),
                    None,
                ),
                step(
                    "guide",
                    "Update the parity guide",
                    "in_progress",
                    None,
                    None,
                ),
                step(
                    "redirect",
                    "Decide where old links go",
                    "waiting",
                    None,
                    Some("child-question-docs"),
                ),
                step("rerun", "Rerun the link check", "pending", None, None),
                step(
                    "publish",
                    "Publish the guide",
                    "blocked",
                    Some("Needs the redirect decision and a passing link check"),
                    None,
                ),
            ],
        }),
        stale: false,
        continued: None,
        history: vec![
            PlanChange {
                at: at(30),
                revision: 9,
                summary: "P-2: 2/2 done, 0 dropped".into(),
            },
            PlanChange {
                at: at(41),
                revision: 6,
                summary: "P-1: 3/4 done, 1 dropped".into(),
            },
        ],
    };
    parts.models.refreshed_at = Some(at(28));
    let host_commands = std::mem::take(&mut parts.commands.commands);
    parts.commands.commands = vec![
        command(
            "release-note",
            "Format a release note for a named change",
            CommandSource::Extension,
            Some("<change>"),
        ),
        command(
            "parity-summary",
            "Summarize the build and its remaining work",
            CommandSource::Prompt,
            None,
        ),
        command(
            "skill:migration-guide",
            "Load the migration guide checklist",
            CommandSource::Skill,
            None,
        ),
    ];
    parts.commands.commands.extend(host_commands);
    parts.board = BuildBoard {
        tasks: vec![
            task(
                "T-004",
                "Publish the guide",
                ("blocked", "blocked"),
                &["T-003"],
                Some("Waiting for the link check"),
                at(19),
            ),
            task(
                "T-003",
                "Check legacy links",
                ("dropped", "failed"),
                &["T-001"],
                Some("The old guide URL did not resolve"),
                at(20),
            ),
            task(
                "T-002",
                "Draft migration notes",
                ("running", "running"),
                &["T-001"],
                None,
                at(22),
            ),
            task(
                "T-001",
                "Record the baseline",
                ("done", "landed"),
                &[],
                None,
                at(27),
            ),
        ],
        proposed: Some(proposal(place.project.clone())),
        unavailable: None,
    };
    record(&mut parts, &at, (&first, &second));
    parts.events.clear();
    for (minutes, kind, words) in [
        (50, "ready", "{}".to_owned()),
        (28, "child_started", r#"{"child":"migration"}"#.to_owned()),
        (
            25,
            "job_control",
            r#"{"job":"j1-1","action":"background","state":"running"}"#.to_owned(),
        ),
        (21, "turn_ended", r#"{"turn":3,"aborted":false}"#.to_owned()),
        (
            20,
            "card_opened",
            r#"{"card":"child-question-docs"}"#.to_owned(),
        ),
    ] {
        parts.event(&at(minutes), kind, words);
    }
    parts
}

/// The build record: two commits, a failed and a passed gate, one review round, one decision.
fn record(parts: &mut Parts, at: &dyn Fn(u64) -> String, (first, second): (&str, &str)) {
    let record = &mut parts.record;
    record.title = "Guide and migration".into();
    record.outcome.head = Some(second.into());
    record.diff = Some(BuildDiff {
        commits: 2,
        files: 3,
        added: 23,
        removed: 3,
    });
    record.commits = vec![
        BuildCommit {
            sha: first.into(),
            subject: "Rename the parity guide".into(),
            slice: Some("docs".into()),
        },
        BuildCommit {
            sha: second.into(),
            subject: "Add the compatibility note".into(),
            slice: Some("migration".into()),
        },
    ];
    record.commits_total = 2;
    record.verification = vec![
        BuildVerification {
            attempt: "G1".into(),
            tier: "full".into(),
            head: first.into(),
            result: "fail".into(),
            failed_command: Some("test -f docs/compatibility.md".into()),
            started: at(27),
            ended: at(27),
        },
        BuildVerification {
            attempt: "G2".into(),
            tier: "full".into(),
            head: second.into(),
            result: "pass".into(),
            failed_command: None,
            started: at(22),
            ended: at(21),
        },
    ];
    record.reviews = vec![BuildReview {
        round: 1,
        reviewer: "review".into(),
        model: OPUS.into(),
        head: first.into(),
        verdict: "changes".into(),
        at: at(23),
        summary: Some("Resolve the compatibility gaps before publishing.".into()),
        findings: vec![
            BuildFinding {
                id: "F1".into(),
                severity: "major".into(),
                path: Some("docs/parity-guide.md".into()),
                line: Some(4),
                text: "The old URL needs a redirect decision.".into(),
                resolution: None,
            },
            BuildFinding {
                id: "F2".into(),
                severity: "minor".into(),
                path: Some("line.txt".into()),
                line: Some(12),
                text: "The old label needs a migration note.".into(),
                resolution: Some(BuildResolution {
                    state: "fixed".into(),
                    commit: Some(second.into()),
                    reason: Some("Added docs/compatibility.md".into()),
                }),
            },
        ],
    }];
    record.decisions = vec![BuildDecision {
        at: at(22),
        kind: "question".into(),
        text: "Publish before the compatibility review?".into(),
        answer: Some("Wait for the review".into()),
    }];
    record.timeline = [
        (50, "started", None),
        (27, "verify", Some("G1 failed")),
        (23, "review", Some("Changes requested")),
        (21, "verify", Some("G2 passed")),
    ]
    .into_iter()
    .map(|(minutes, phase, detail)| BuildTimeline {
        at: at(minutes),
        phase: phase.into(),
        detail: detail.map(str::to_owned),
    })
    .collect();
    record.usage = BuildUsageTotals {
        parent: totals(11_100, 900, Some(0.09)),
        children: totals(5_100, 550, Some(0.03)),
        reviewers: totals(1_600, 200, Some(0.02)),
        total: totals(17_800, 1_650, Some(0.14)),
    };
}

/// The three children: migration writing the notes, docs asking the owner, review done.
fn children(at: &dyn Fn(u64) -> String, ms: &dyn Fn(u64) -> u64, reviewed: &str) -> Vec<Child> {
    let models = vec![PARENT.to_owned(), OPUS.to_owned()];
    let profiles = vec![
        profile("default", PARENT, "medium", "Parent's model"),
        profile("review-subscription", OPUS, "high", "Subscription review"),
        profile(
            "docs-api",
            OPUS_API,
            "high",
            "API profile; not approved for additional agents",
        ),
    ];
    let detail = |id: &str, kind: &str, title: &str, brief: &str| SessionChildDetail {
        id: id.into(),
        kind: kind.into(),
        title: title.into(),
        brief: brief.into(),
        brief_more: false,
        owns: Vec::new(),
        after: Vec::new(),
        profile: "review-subscription".into(),
        model: OPUS.into(),
        pending_model: None,
        effort: Some("high".into()),
        state: "running".into(),
        created_at: ms(29),
        started_at: None,
        ended_at: None,
        activity: None,
        edits: Vec::new(),
        usage: ChildUsage {
            input: 0,
            output: 0,
            cost_usd: None,
            context_percent: None,
            turns: 0,
        },
        steers: Vec::new(),
        question: None,
        report: None,
        report_more: false,
        error: None,
        verdict: None,
        pulse: Vec::new(),
        history: String::new(),
        dropped: None,
        transcript: true,
        models: models.clone(),
        profiles: profiles.clone(),
    };
    let migration_brief = "Preview · sample brief. Write the compatibility and migration notes. Cover the original label, the new label, and how existing projects can update without losing context. Start by reading line.txt and the draft guide. Keep the compatibility note near the start, cite which examples still use the old name, and tell the parent when the notes are ready for review.";
    let migration = Child {
        detail: SessionChildDetail {
            brief_more: true,
            owns: vec!["docs/migrations/".into()],
            profile: "default".into(),
            model: PARENT.into(),
            pending_model: Some(OPUS.into()),
            effort: Some("medium".into()),
            started_at: Some(ms(28)),
            activity: Some("edit docs/compatibility.md".into()),
            edits: vec![ChildEdit {
                path: "docs/compatibility.md".into(),
                outside: true,
                at: at(24),
            }],
            usage: ChildUsage {
                input: 2_900,
                output: 250,
                cost_usd: Some(0.03),
                context_percent: Some(7.0),
                turns: 2,
            },
            steers: vec![
                steer(at(27), "Use the new label throughout the examples."),
                steer(at(25), "Mention who still needs the old name."),
            ],
            pulse: vec![0, 1, 2, 2, 1],
            history: "read · edit · running".into(),
            ..detail(
                "migration",
                "slice",
                "Write migration notes",
                migration_brief,
            )
        },
        transcript: vec![
            asked(&at(28), migration_brief, InputOrigin::Auto),
            said(
                &at(28),
                "assistant",
                Some(PARENT),
                vec![
                    text(
                        "Preview · sample reply. Reading the label's uses before writing the notes.",
                    ),
                    tool(
                        "migration-read",
                        "read",
                        "line.txt",
                        &["14 lines", "original label"],
                    ),
                ],
            ),
            asked(
                &at(27),
                "Use the new label throughout the examples.",
                InputOrigin::Owner,
            ),
            said(
                &at(24),
                "assistant",
                Some(PARENT),
                vec![
                    text(
                        "Preview · sample reply. The note belongs beside the guide, so it lands outside docs/migrations/; I told the parent.",
                    ),
                    tool(
                        "migration-edit",
                        "edit",
                        "docs/compatibility.md",
                        &["+20 −0"],
                    ),
                ],
            ),
        ],
    };
    let docs_brief = "Preview · sample brief. Rename the draft guide and refresh the opening example. Ask before removing the old path.";
    let docs = Child {
        detail: SessionChildDetail {
            owns: vec!["docs/parity-guide.md".into()],
            state: "needs".into(),
            started_at: Some(ms(27)),
            edits: vec![ChildEdit {
                path: "docs/parity-guide.md".into(),
                outside: false,
                at: at(22),
            }],
            usage: ChildUsage {
                input: 2_200,
                output: 300,
                cost_usd: None,
                context_percent: Some(9.0),
                turns: 3,
            },
            steers: vec![steer(at(23), "Keep existing links working if practical.")],
            question: Some(ChildQuestion {
                question: "Keep the old guide as a redirect?".into(),
                options: vec![
                    ChildQuestionOption {
                        label: "Keep a redirect".into(),
                        detail: Some("Preserve existing links".into()),
                    },
                    ChildQuestionOption {
                        label: "Use only the new name".into(),
                        detail: None,
                    },
                ],
                asked_at: at(20),
            }),
            pulse: vec![0, 1, 2, 0],
            history: "read · edit · asks owner".into(),
            ..detail("docs", "slice", "Update the parity guide", docs_brief)
        },
        transcript: vec![
            asked(&at(27), docs_brief, InputOrigin::Auto),
            asked(
                &at(23),
                "Keep existing links working if practical.",
                InputOrigin::Owner,
            ),
            said(
                &at(22),
                "assistant",
                Some(OPUS),
                vec![
                    text(
                        "Preview · sample reply. Renamed the guide and refreshed its opening example.",
                    ),
                    tool(
                        "docs-move",
                        "bash",
                        "git mv docs/draft-guide.md docs/parity-guide.md",
                        &[],
                    ),
                ],
            ),
            said(
                &at(20),
                "assistant",
                Some(OPUS),
                vec![text(
                    "Preview · sample reply. Old links point at docs/draft-guide.md. Asking the owner whether to keep it as a redirect.",
                )],
            ),
        ],
    };
    let review_brief = "Preview · sample brief. Review the label migration for broken links and stale examples. Do not edit files.";
    let report = "Preview · sample report. The new label is consistent. The guide needs a compatibility note and a decision on preserving the old link.";
    let review = Child {
        detail: SessionChildDetail {
            created_at: ms(26),
            state: "done".into(),
            started_at: Some(ms(26)),
            ended_at: Some(ms(23)),
            usage: ChildUsage {
                input: 1_600,
                output: 200,
                cost_usd: Some(0.02),
                context_percent: None,
                turns: 1,
            },
            steers: vec![steer(at(25), "Include a check for the old guide's URL.")],
            report: Some(report.into()),
            verdict: Some(ReviewVerdict {
                verdict: "changes".into(),
                summary: "Resolve the compatibility gaps before publishing.".into(),
                findings: vec![
                    ReviewFinding {
                        severity: "major".into(),
                        path: Some("docs/parity-guide.md".into()),
                        line: Some(4),
                        text: "The guide does not say where old links should redirect.".into(),
                    },
                    ReviewFinding {
                        severity: "minor".into(),
                        path: Some("line.txt".into()),
                        line: Some(12),
                        text: "The old label has no migration note.".into(),
                    },
                ],
                head: Some(reviewed.into()),
                at: at(23),
            }),
            pulse: vec![1, 2, 1, 0],
            history: "read · changes requested".into(),
            models: Vec::new(),
            profiles: Vec::new(),
            ..detail(
                "review",
                "review",
                "Review the guide and migration",
                review_brief,
            )
        },
        transcript: vec![
            asked(&at(26), review_brief, InputOrigin::Auto),
            said(
                &at(23),
                "assistant",
                Some(OPUS),
                vec![
                    tool("review-read", "read", "docs/parity-guide.md", &["38 lines"]),
                    text(report),
                ],
            ),
        ],
    };
    vec![migration, docs, review]
}

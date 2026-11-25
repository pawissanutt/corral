//! The sample build session as `packages/corral-pi/test/fixtures/sessions/parts.json` tells it
//! (rename the draft guide to the parity guide and write migration notes for the old label):
//! its parent transcript, the three cards it waits on (docs asks the owner, a proposed board
//! plan, the parent's own two-question card), the two follow-ups the owner queued, and its parts
//! (`parts_story.rs`). A build started in the preview gets the parts a new host has: no
//! children, jobs or changes yet, the model catalog and the host's own commands. Every word is
//! labelled sample content where it could pass for a model's.

use corral_protocol::detail::*;
use corral_types::{Origin, TaskDifficulty};

use super::parts::{Parts, WINDOW};
use super::parts_story::{sample, text, tool};
use super::pi::{Input, question_card};
use super::sample::{MINUTE, sha};
use super::stamp;
use crate::model::daemon::Feed;

pub(super) const PARENT: &str = "openai-codex/gpt-6.1-sol";
pub(super) const OPUS: &str = "anthropic/claude-opus-4-8";
pub(super) const OPUS_API: &str = "anthropic-api/claude-opus-4-8";

/// Where a build works: its worktree, branch, base and project, from its row.
pub(super) struct Place {
    pub(super) worktree: String,
    pub(super) branch: String,
    pub(super) base: String,
    pub(super) project: Option<String>,
}

fn place(feed: &Feed, id: &str) -> Place {
    let build = feed.assistants.get(id).and_then(|row| row.build.as_ref());
    Place {
        worktree: build.map_or_else(String::new, |build| build.worktree.clone()),
        branch: build
            .and_then(|build| build.branch.clone())
            .unwrap_or_else(|| "HEAD".into()),
        base: build
            .and_then(|build| build.base.clone())
            .unwrap_or_else(|| "main".into()),
        project: build.and_then(|build| build.project.clone()),
    }
}

/// The models a sample build may use: two providers of one vendor (the subscription and the
/// API, which are not each other's fallback) and another vendor's.
fn catalog(at: String) -> SessionModels {
    let model = |id: &str, provider: &str, name: &str, levels: &[&str], scoped, note: Option<_>| {
        ModelChoice {
            id: id.into(),
            provider: provider.into(),
            name: Some(name.into()),
            context_window: Some(WINDOW),
            effort_levels: levels.iter().map(|level| (*level).to_owned()).collect(),
            scoped,
            favorite: false,
            note: note.map(str::to_owned),
        }
    };
    SessionModels {
        current: None,
        default: Some(PARENT.into()),
        effort: None,
        models: vec![
            model(
                OPUS,
                "anthropic",
                "Claude Opus 4.8 (subscription)",
                &["low", "medium", "high"],
                true,
                None,
            ),
            model(
                OPUS_API,
                "anthropic-api",
                "Claude Opus 4.8 (API)",
                &["low", "medium", "high"],
                false,
                Some("API access, not a subscription fallback"),
            ),
            model(
                PARENT,
                "openai-codex",
                "GPT-6.1 Sol",
                &["low", "medium", "high", "xhigh"],
                true,
                None,
            ),
        ],
        refreshed_at: Some(at),
        refreshing: false,
    }
}

pub(super) fn command(
    name: &str,
    description: &str,
    source: CommandSource,
    args: Option<&str>,
) -> SlashCommand {
    SlashCommand {
        name: name.into(),
        description: description.into(),
        source,
        args_hint: args.map(str::to_owned),
    }
}

/// The commands every build host carries.
fn host_commands() -> Vec<SlashCommand> {
    vec![
        command(
            "compact",
            "Compact session context",
            CommandSource::Host,
            Some("[instructions]"),
        ),
        command(
            "corral-export",
            "Export this session",
            CommandSource::Host,
            None,
        ),
        command(
            "board",
            "Refresh the build board",
            CommandSource::Host,
            None,
        ),
    ]
}

pub(super) fn usage_row(
    key: &str,
    name: &str,
    kind: &str,
    input: u64,
    output: u64,
    cost: Option<f64>,
) -> UsageRow {
    UsageRow {
        key: key.into(),
        name: name.into(),
        kind: kind.into(),
        tokens: input + output,
        input,
        output,
        cost_usd: cost,
        context_percent: None,
        turns: 0,
    }
}

pub(super) fn totals(input: u64, output: u64, cost: Option<f64>) -> BuildUsage {
    BuildUsage {
        input,
        output,
        cost_usd: cost,
    }
}

/// What a new build's host serves before it has done anything.
pub(super) fn fresh(feed: &Feed, id: &str) -> Parts {
    let place = place(feed, id);
    let at = stamp(feed.now);
    let name = feed
        .assistants
        .get(id)
        .map_or_else(|| id.to_owned(), |row| row.title.clone());
    let mut parts = Parts {
        children: Vec::new(),
        messages: Vec::new(),
        jobs: Vec::new(),
        changes: SessionChanges {
            comparison: ChangesComparison::Session,
            comparisons: vec![ComparisonInfo {
                id: ChangesComparison::Session,
                label: "This session".into(),
                unavailable: None,
            }],
            scopes: Vec::new(),
            scope: None,
            root: place.worktree.clone(),
            base: None,
            head: None,
            files: Vec::new(),
            total: 0,
            added: 0,
            removed: 0,
            truncated: false,
            error: None,
        },
        patches: Vec::new(),
        pane: usage_row("pane", "Pane", "session", 0, 0, None),
        helpers: Vec::new(),
        context_tokens: 0,
        context: SessionContext {
            cwd: place.worktree.clone(),
            worktree: Some(place.worktree.clone()),
            branch: Some(place.branch.clone()),
            context_files: vec![format!("{}/AGENTS.md", place.worktree)],
            skills: Vec::new(),
            prompts: Vec::new(),
            tools: [
                "read",
                "edit",
                "bash",
                "ask_user",
                "task",
                "build_verify",
                "board_plan",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            mcp: Vec::new(),
        },
        plan: SessionPlan {
            plan: None,
            stale: false,
            continued: None,
            history: Vec::new(),
        },
        models: catalog(at.clone()),
        commands: SessionCommands {
            commands: host_commands(),
        },
        board: BuildBoard {
            tasks: Vec::new(),
            proposed: None,
            unavailable: None,
        },
        record: BuildRecord {
            title: name,
            outcome: BuildOutcome {
                state: "open".into(),
                at: None,
                head: None,
                landed: None,
                base_branch: Some(place.base),
                override_reason: None,
            },
            diff: None,
            commits: Vec::new(),
            commits_total: 0,
            verification: Vec::new(),
            reviews: Vec::new(),
            decisions: Vec::new(),
            timeline: Vec::new(),
            usage: BuildUsageTotals {
                parent: totals(0, 0, None),
                children: totals(0, 0, None),
                reviewers: totals(0, 0, None),
                total: totals(0, 0, None),
            },
        },
        plan_mode: None,
        plan_doc: String::new(),
        events: Vec::new(),
    };
    parts.event(&at, "ready", "{}".into());
    parts.record.timeline.push(BuildTimeline {
        at,
        phase: "started".into(),
        detail: None,
    });
    parts
}

/// The board plan the sample build proposes: publish the guide, then audit links to it.
pub(super) fn proposal(project: Option<String>) -> BoardPlan {
    let body = "Preview · sample. Publish the updated guide after the owner confirms the redirect policy. Keep the migration example synchronized with line.txt. Check the old and new URLs in the published preview, then attach the output of the link check and record which URL is canonical. The original task body continues with examples and a release checklist.";
    BoardPlan {
        title: "Follow up on the parity guide".into(),
        project,
        why:
            "Publish after choosing a redirect, then verify old links against the published guide."
                .into(),
        tasks: vec![
            BoardTaskPlan {
                key: "publish".into(),
                project: None,
                title: "Publish the updated guide".into(),
                kind: Some("task".into()),
                size: Some("s".into()),
                difficulty: Some(TaskDifficulty::Standard),
                tags: vec!["documentation".into()],
                profile: Some("parity".into()),
                depends: Vec::new(),
                body: body.into(),
                body_bytes: 3312,
            },
            BoardTaskPlan {
                key: "audit".into(),
                project: None,
                title: "Audit legacy links".into(),
                kind: None,
                size: None,
                difficulty: None,
                tags: Vec::new(),
                profile: None,
                depends: vec!["publish".into()],
                body: "Audit links to the guide.".into(),
                body_bytes: 25,
            },
        ],
    }
}

/// The question `docs` asks the owner, as the host docks it.
fn child_question(at: String) -> SessionCard {
    SessionCard {
        id: "child-question-docs".into(),
        kind: CardKind::ChildQuestion,
        title: "Keep the old guide as a redirect?".into(),
        meta: Some(format!("review-subscription · {OPUS}")),
        body: None,
        body_more: false,
        questions: vec![CardQuestion {
            header: None,
            text: "Keep the old guide as a redirect?".into(),
            options: vec![
                option(
                    "redirect",
                    "Keep a redirect",
                    Some("Preserve existing links"),
                ),
                option("rename", "Use only the new name", None),
            ],
            initial: None,
            multi: false,
            free_text: true,
        }],
        child: Some("docs".into()),
        tool: None,
        opened_at: at,
        expires_at: None,
    }
}

/// The board plan card: File and start, File only, Dismiss.
fn plan_card(at: String, project: Option<&str>) -> SessionCard {
    SessionCard {
        id: "board-plan-followup".into(),
        kind: CardKind::BoardPlan,
        title: "Follow up on the parity guide".into(),
        meta: Some(format!("{} · 2 tasks", project.unwrap_or("this workspace"))),
        body: Some("Publish the updated guide, then audit links to it.".into()),
        body_more: false,
        questions: vec![CardQuestion {
            header: None,
            text: "File this plan on the board?".into(),
            options: vec![
                option(
                    "file_start",
                    "File and start",
                    Some("File these tasks and queue them"),
                ),
                option(
                    "file",
                    "File only",
                    Some("File these tasks without starting them"),
                ),
                option(
                    "dismiss",
                    "Dismiss",
                    Some("Discard the proposal without filing"),
                ),
            ],
            initial: None,
            multi: false,
            free_text: false,
        }],
        child: None,
        tool: Some("board_plan".into()),
        opened_at: at,
        expires_at: None,
    }
}

fn option(id: &str, label: &str, detail: Option<&str>) -> CardOption {
    CardOption {
        id: id.into(),
        label: label.into(),
        detail: detail.map(str::to_owned),
        unavailable: None,
        needs_text: None,
        destructive: false,
    }
}

/// The sample build's parent transcript, the cards it waits on, the two follow-ups the owner
/// queued, and its parts.
pub(super) fn seed(feed: &mut Feed, id: &str) {
    let now = feed.now;
    let at = |minutes: u64| stamp(now.saturating_sub(minutes * MINUTE));
    let place = place(feed, id);
    let parts = sample(feed, id, &place);
    let first = sha("parity/docs", 7);
    let second = sha("parity/migration", 7);
    let Some(host) = feed.hosts.get_mut(id) else {
        return;
    };
    host.record_turn(
        &at(50),
        &["Rename the draft guide to the parity guide and write migration notes for the old label."],
        vec![
            text("Preview · sample build. Two slices and a review: docs renames the guide, migration writes the compatibility notes, review checks both. Starting docs and migration now."),
            tool("seed-read-line", "read", "line.txt", &["14 lines", "original label"]),
        ],
    );
    host.record_turn(
        &at(26),
        &[format!("[corral] Gate G1 failed on {first}: test -f docs/compatibility.md").as_str()],
        vec![
            text("Preview · sample turn. The gate wants the compatibility note before anything lands; migration is writing it while review starts on the rename."),
            tool(
                "seed-check-note",
                "bash",
                "test -f docs/compatibility.md",
                &["Check failed: missing note."],
            ),
        ],
    );
    host.record_turn(
        &at(21),
        &[format!("[corral] Gate G2 passed on {second}; review asks for changes: 2 findings").as_str()],
        vec![text("Preview · sample turn. G2 passed with the note in place. One major finding stays open: where old links go. docs is asking you that, and I have proposed the follow-up on the board.")],
    );
    host.pending.push(child_question(at(20)));
    host.pending
        .push(plan_card(at(19), place.project.as_deref()));
    let card = super::build_ops::api_question(&at(18));
    host.transcript.push(question_card(
        &at(18),
        &card,
        "pending",
        serde_json::Value::Null,
    ));
    host.pending.push(card);
    for (key, words) in [
        ("notes", "Check the migration notes."),
        ("recap", "Send a short status recap."),
    ] {
        host.queued.push(Input {
            text: words.into(),
            origin: Origin::Owner,
            entry: Some(super::pi::input_entry(id, key)),
        });
    }
    host.parts = Some(Box::new(parts));
    let model = feed.assistants.get(id).and_then(|row| row.model.clone());
    if let Some(host) = feed.hosts.get_mut(id) {
        super::plan_mode::seed(host, model.as_deref(), at(17));
    }
}

/// A host that a build session runs on gets its parts once its row is recorded.
pub(super) fn attach(feed: &mut Feed, id: &str) {
    let parts = fresh(feed, id);
    if let Some(host) = feed.hosts.get_mut(id) {
        host.parts = Some(Box::new(parts));
    }
}

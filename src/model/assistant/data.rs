//! Snapshot fixtures. Message text, profiles, timings and filed ids are illustrative.

use crate::model::effect::Page;

/// Shared quick-thread id; its composer uses the Assistant | PM switch, not named-session mode.
pub const QUICK_THREAD: &str = "A-7";

/// A selectable profile, its real model id and the note shown in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Profile name on the pill.
    pub name: &'static str,
    /// Model id and effort under the name.
    pub model: &'static str,
    /// Short selection advice.
    pub note: &'static str,
}

/// Profiles in `[assistant.quick] models`, in cycling order.
pub const MODELS: &[Model] = &[
    Model {
        name: "pi-sol xhigh",
        model: "gpt-6.1-sol · xhigh",
        note: "assistant profile · default",
    },
    Model {
        name: "pi-terra high",
        model: "gpt-5.6-terra · high",
        note: "cheaper",
    },
    Model {
        name: "pi-luna",
        model: "gpt-6-luna · low",
        note: "cheapest · also writes roll summaries",
    },
    Model {
        name: "pi-opus high",
        model: "claude-opus-4-5 · high",
        note: "Anthropic",
    },
];

/// The question that answers and files in the same turn.
pub const BOTH_Q: &str = "why is T-867 stale, and what's blocking T-870? we should cache gate receipts so a base move doesn't force a full re-gate";

/// A plain or linked run of the static streamed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerRun {
    /// Plain prose.
    Text(&'static str),
    /// A mono reference that opens its record.
    Link(&'static str, Page),
}

/// A paragraph or numbered list revealed as one streaming step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerBlock {
    /// A paragraph with linked records.
    Paragraph(&'static [AnswerRun]),
    /// Numbered list entries, each with linked records.
    Ordered(&'static [&'static [AnswerRun]]),
}

use AnswerRun::{Link, Text};

/// The complete answer, divided into the mockup's four reveal blocks.
pub const ANSWER: &[AnswerBlock] = &[
    AnswerBlock::Paragraph(&[
        Link("T-867", Page::Task("T-867")),
        Text(" is stale because the base moved. Its gate ran on candidate "),
        Link("5949a4b", Page::Task("T-867")),
        Text(
            "; main moved at 22:05, so that verdict now belongs to an older candidate. Corral queued a re-gate on the rebased candidate, so nothing is needed from you there.",
        ),
    ]),
    AnswerBlock::Paragraph(&[
        Text("Plan "),
        Link("T-870", Page::Plan("T-870")),
        Text(" has three blockers:"),
    ]),
    AnswerBlock::Ordered(&[
        &[
            Link("T-867", Page::Task("T-867")),
            Text(" itself: both "),
            Link("T-868", Page::Task("T-868")),
            Text(" and "),
            Link("T-869", Page::Task("T-869")),
            Text(" build on it."),
        ],
        &[
            Link("T-868", Page::Task("T-868")),
            Text(" waits on your answer to "),
            Link("T-868/R1/Q1", Page::Question),
            Text(" (restore policy). The worker recommends option 1."),
        ],
        &[Text(
            "None of its 15 acceptance scenarios is mapped to a member, so final verification has no owner yet.",
        )],
    ]),
    AnswerBlock::Paragraph(&[
        Text("Your caching idea is filed as "),
        Link("T-903", Page::Task("T-903")),
        Text(". I checked duplicates first: "),
        Link("T-852", Page::Task("T-852")),
        Text(
            " handles the same trigger (a gate gone stale after a base move) but does not cache, so I filed it separately.",
        ),
    ]),
];

/// A record cited under the completed reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    /// Link text, including the command used.
    pub label: &'static str,
    /// Destination, absent for the illustrative search result.
    pub page: Option<Page>,
    /// Search outcome shown when there is no destination.
    pub toast: Option<&'static str>,
}

/// The answer's five supporting records and commands.
pub const SOURCES: &[Source] = &[
    Source {
        label: "corral show T-867",
        page: Some(Page::Task("T-867")),
        toast: None,
    },
    Source {
        label: "run R1 of T-867",
        page: Some(Page::Session("T-867")),
        toast: None,
    },
    Source {
        label: "corral plan progress T-870",
        page: Some(Page::Plan("T-870")),
        toast: None,
    },
    Source {
        label: "corral question show T-868/R1/Q1",
        page: Some(Page::Question),
        toast: None,
    },
    Source {
        label: "corral list --search receipt",
        page: None,
        toast: Some("duplicate search: 2 candidates, none close"),
    },
];

/// One expandable tool-activity row: tool, command and elapsed time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// Tool name.
    pub kind: &'static str,
    /// Command sent to the tool.
    pub command: &'static str,
    /// Duration of this call.
    pub elapsed: &'static str,
}

/// Tool activity retained even for a stopped partial answer.
pub const TOOLS: &[Tool] = &[
    Tool {
        kind: "Bash",
        command: "corral show T-867 --json",
        elapsed: "0.4 s",
    },
    Tool {
        kind: "Bash",
        command: "corral log T-867 --since 20:40",
        elapsed: "0.2 s",
    },
    Tool {
        kind: "Bash",
        command: "corral plan progress T-870",
        elapsed: "0.6 s",
    },
    Tool {
        kind: "Bash",
        command: "corral question show T-868/R1/Q1",
        elapsed: "0.3 s",
    },
    Tool {
        kind: "Bash",
        command: "corral list --search \"gate receipt\" --json",
        elapsed: "0.3 s",
    },
    Tool {
        kind: "Bash",
        command: "corral new --idea --project corral …",
        elapsed: "0.5 s",
    },
];

/// An Idea's unchanged request and the fields the assistant wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filed {
    /// Filed task id.
    pub id: &'static str,
    /// Filing time.
    pub at: &'static str,
    /// Device that authored the request.
    pub device: &'static str,
    /// Owner's words, never rewritten.
    pub words: &'static str,
    /// Assistant-written title.
    pub title: &'static str,
    /// Assistant-written labels.
    pub labels: &'static [&'static str],
    /// Related task, when a duplicate was kept separate.
    pub related: Option<&'static str>,
    /// Assistant-written acceptance criteria.
    pub criteria: &'static [&'static str],
    /// Duplicate-check outcome.
    pub duplicates: &'static str,
}

/// The three full filed-card fixtures.
pub const FILED: &[Filed] = &[
    Filed {
        id: "T-901",
        at: "22:04",
        device: "pawis-desktop",
        words: "review diffs need an inline comment box tied to the candidate SHA",
        title: "Review: inline comments bound to the candidate SHA",
        labels: &["review", "ui"],
        related: Some("T-874"),
        criteria: &[
            "A comment on a diff line records the candidate SHA it was written against.",
            "A newer candidate shows the comment as stale, not deleted.",
            "Comments appear in the Review screen on desktop and phone.",
        ],
        duplicates: "asked first: close to T-874; you chose to file separately (related link kept)",
    },
    Filed {
        id: "T-902",
        at: "22:10",
        device: "pawis-iphone",
        words: "board: show the PM inbox count in the title bar",
        title: "Board: PM inbox count in the title bar",
        labels: &["board", "pm"],
        related: None,
        criteria: &[
            "The title bar shows open PM inbox items as a count next to the health line.",
            "The count updates within one sync pass.",
        ],
        duplicates: "checked: nothing close",
    },
    Filed {
        id: "T-903",
        at: "22:11",
        device: "pawis-desktop",
        words: "we should cache gate receipts so a base move doesn't force a full re-gate",
        title: "Gate: reuse receipts for unchanged crates after a base move",
        labels: &["gate", "performance"],
        related: None,
        criteria: &[
            "A re-gate after a base move that touches no crate in the candidate's write scope reuses the earlier receipts and records why.",
            "A base move that touches any crate in scope runs the full tier.",
            "The new gate receipt names every reused receipt id.",
        ],
        duplicates: "checked: T-852 shares the trigger, not the fix",
    },
];

/// One Filed from chat row, always naming its source session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiledRow {
    /// Task to open.
    pub id: &'static str,
    /// Filed title or merge outcome.
    pub title: &'static str,
    /// Filing time.
    pub at: &'static str,
    /// Session and host that supplied the words.
    pub source: &'static str,
}

/// Filed rows in newest-first order.
pub const FILED_LIST: &[FiledRow] = &[
    FiledRow {
        id: "T-903",
        title: "Gate: reuse receipts for unchanged crates after a base move",
        at: "22:11",
        source: "A-7 · quick thread",
    },
    FiledRow {
        id: "T-902",
        title: "Board: PM inbox count in the title bar",
        at: "22:10",
        source: "A-7 · quick thread",
    },
    FiledRow {
        id: "T-874",
        title: "merged · comment on Pi review annotations",
        at: "22:04",
        source: "A-7 · quick thread",
    },
    FiledRow {
        id: "T-899",
        title: "PM read view lists assistant inbox items",
        at: "yesterday",
        source: "A-17 · pane w9A:p1 (corral pi)",
    },
];

/// A daemon or Pi-pane session listed inside Agents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Session {
    /// Session identity.
    pub id: &'static str,
    /// Session title.
    pub title: &'static str,
    /// State including its since-time where live.
    pub state: &'static str,
    /// Host description.
    pub location: &'static str,
    /// Whether the app can select this host's model.
    pub daemon: bool,
    /// Focus task, absent for task-less sessions.
    pub focus: Option<&'static str>,
    /// Today's dollar cost.
    pub cost: f32,
    /// Scratch count.
    pub scratch: u32,
}

/// All sessions, including the named daemon session A-18.
pub const SESSIONS: &[Session] = &[
    Session {
        id: "A-7",
        title: "Quick thread",
        state: "live · since 14:02",
        location: "runs: daemon",
        daemon: true,
        focus: None,
        cost: 1.12,
        scratch: 0,
    },
    Session {
        id: "A-15",
        title: "T-854 co-work check",
        state: "live · since 21:02",
        location: "pane w15C:p2 · corral pi --task T-854",
        daemon: false,
        focus: Some("T-854"),
        cost: 2.18,
        scratch: 0,
    },
    Session {
        id: "A-17",
        title: "Board TUI mockup: async loading and settings",
        state: "open",
        location: "pane w9A:p1 · corral pi",
        daemon: false,
        focus: None,
        cost: 6.40,
        scratch: 1,
    },
    Session {
        id: "A-12",
        title: "Co-work C2 follow-ups",
        state: "open · task done",
        location: "pane w9A:p2 · corral pi --task T-853",
        daemon: false,
        focus: Some("T-853"),
        cost: 0.88,
        scratch: 0,
    },
    Session {
        id: "A-11",
        title: "PM read contract questions",
        state: "ended",
        location: "−",
        daemon: false,
        focus: Some("T-890"),
        cost: 0.52,
        scratch: 0,
    },
    Session {
        id: "A-18",
        title: "Changes review notes",
        state: "live · since 20:31",
        location: "runs: daemon",
        daemon: true,
        focus: None,
        cost: 0.41,
        scratch: 0,
    },
];

/// Finds a known session without inventing a host for an unknown id.
pub fn session(id: &str) -> Option<&'static Session> {
    SESSIONS.iter().find(|s| s.id == id)
}

/// Finds the full assistant-written fields of a filed card.
pub fn filed(id: &str) -> Option<&'static Filed> {
    FILED.iter().find(|f| f.id == id)
}

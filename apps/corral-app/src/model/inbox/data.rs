//! The question projection and its explicitly illustrative supporting artifacts.

/// One answer and its consequences, shown without fetching task detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionData {
    /// The radio option's complete label.
    pub label: &'static str,
    /// What happens after choosing it, under “Then”.
    pub next: &'static str,
    /// Work released by this answer, under “Unblocks”.
    pub unblocks: &'static str,
    /// Estimated effort and tradeoffs, under “Costs”.
    pub costs: &'static str,
    /// The evidence link's name after “see”.
    pub proof: &'static str,
    /// Whether to show “worker recommends”.
    pub rec: bool,
}

/// The shape of a dependency mark in “Waiting on this answer”.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// Needs the owner's answer.
    You,
    /// A plan dependency, shown with the not-yet mark.
    Plan,
    /// Independent information, shown dim with the not-applicable mark.
    Info,
}

/// One dependency in the situation band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    /// Task or plan id.
    pub id: &'static str,
    /// Dependency explanation.
    pub text: &'static str,
    /// The mark and emphasis of this dependency.
    pub kind: WaitKind,
}

/// One illustrative message in the question thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    /// Speaker shown in bold.
    pub who: &'static str,
    /// Device attribution, when present.
    pub via: Option<&'static str>,
    /// Message time.
    pub at: &'static str,
    /// Message body.
    pub text: &'static str,
    /// Whether the source labels this message illustrative.
    pub illustrative: bool,
}

/// One previous attempt or owner decision in History.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The date, plan revision, checkpoint or report.
    pub at: &'static str,
    /// “you” or “worker”, determining the history mark.
    pub who: &'static str,
    /// The full previous decision or attempt.
    pub text: &'static str,
}

/// The winning answer received from another surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnsweredElsewhere {
    /// The winning answer's time.
    pub at: &'static str,
    /// The surface name.
    pub where_: &'static str,
    /// Its attribution slug.
    pub via: &'static str,
    /// Zero-based winning option.
    pub option: usize,
}

/// The full board-projected question, available before lazy evidence loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Question {
    /// Question record key.
    pub key: &'static str,
    /// Revision expectation carried by answers.
    pub rev: u32,
    /// Owner-only audience chip.
    pub kind: &'static str,
    /// Whether the question blocks work.
    pub blocking: bool,
    /// Asking task and navigation target.
    pub task: &'static str,
    /// The enclosing plan.
    pub plan: &'static str,
    /// Worker and run attribution.
    pub from: &'static str,
    /// Escalation attribution.
    pub via: &'static str,
    /// Original question time.
    pub asked: &'static str,
    /// Age shown in the list and header.
    pub age: &'static str,
    /// Short title.
    pub short: &'static str,
    /// Complete situation text.
    pub text: &'static str,
    /// The two answer choices, in numeric-key order.
    pub options: &'static [OptionData],
    /// Why the worker recommends option 1.
    pub recommendation: &'static str,
    /// Source references from the projection.
    pub refs: &'static [&'static str],
    /// The question's dependency list.
    pub waits: &'static [Wait],
    /// The illustrative thread.
    pub thread: &'static [Message],
    /// Real prior decisions and attempts.
    pub history: &'static [HistoryEntry],
    /// The alternate winning answer fixture.
    pub answered_elsewhere: AnsweredElsewhere,
}

/// T-868/R1/Q1, including every option, reference and previous decision.
pub const Q: Question = Question {
    key: "T-868/R1/Q1",
    rev: 2,
    kind: "owner only",
    blocking: true,
    task: "T-868",
    plan: "T-870",
    from: "worker T-868 R1",
    via: "escalated by the PM",
    asked: "19:12",
    age: "3h",
    short: "Restore policy: pane-origin input only, or all origins?",
    text: "T-701@P2 leaves one restore-policy decision open for T-868: on stock/plain paths where Esc/dequeue restoration is enabled, should restoration remove only pane-origin queued input, or restore all origins? Rich mode will remain read-only/count-only with no restoration either way.",
    recommendation: "Restore only pane-origin input, so Corral/extension instructions cannot become an owner draft. I can implement the ledger and other controls independently while awaiting this decision.",
    refs: &[
        "T-701@P2",
        "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch",
    ],
    options: &[
        OptionData {
            label: "Restore only pane-origin input; leave Corral/extension items queued",
            rec: true,
            next: "Worker adds the restoreInput hook: pane items return their original text, others stay queued. Then rebase and quick gate.",
            unblocks: "T-868 step 3/5 resumes now; T-870 member 2 of 3 can reach its gate.",
            costs: "≈1 step on pi-sol xhigh (≈$2–4, est.) · Esc on plain paths no longer hands Corral/extension items to your draft.",
            proof: "A · diff hunk",
        },
        OptionData {
            label: "Retain restore-all; journal removals and treat resubmission as new pane input",
            rec: false,
            next: "Worker journals every removal and re-admits restored text as new pane input with fresh identity.",
            unblocks: "Same unblock, later: adds a journal record type and its replay tests.",
            costs: "≈2 steps (≈$6–9, est.) · Corral/extension instructions can land in your draft and be resent as yours.",
            proof: "A · file excerpt",
        },
    ],
    waits: &[
        Wait {
            id: "T-868",
            text: "Blocked until you answer",
            kind: WaitKind::You,
        },
        Wait {
            id: "T-870",
            text: "plan member 2 of 3 · final accept needs it",
            kind: WaitKind::Plan,
        },
        Wait {
            id: "T-869",
            text: "independent · after T-867, not this answer",
            kind: WaitKind::Info,
        },
    ],
    history: &[
        HistoryEntry {
            at: "11-07 · T-701@P2",
            who: "you",
            text: "Approved plan P2: keep main's rich readOnlyQueue; leave the restore-policy choice open only where restoration is enabled.",
        },
        HistoryEntry {
            at: "11-07 · T-701@P2",
            who: "you",
            text: "Asked for a Pi 1.5.0 rebase, not implementation admission.",
        },
        HistoryEntry {
            at: "R1 · checkpoint 2",
            who: "worker",
            text: "Current queue implementation passes typecheck and five deterministic host/control tests (1.64 s).",
        },
        HistoryEntry {
            at: "R1 · report",
            who: "worker",
            text: "Pre-gate queue acceptance evidence and reconciled helper findings; owner restore policy remains pending.",
        },
        HistoryEntry {
            at: "R1 · log",
            who: "worker",
            text: "Native Alt+Up and Esc restore only pane text at 52x24 and 140x46; external identities and draft preserved.",
        },
    ],
    thread: &[
        Message {
            who: "you",
            via: Some("app:pawis-iphone"),
            at: "21:58",
            text: "Does option 1 change anything in rich mode?",
            illustrative: true,
        },
        Message {
            who: "worker T-868",
            via: None,
            at: "21:59",
            text: "No. Rich mode stays read-only/count-only; neither option restores there.",
            illustrative: true,
        },
    ],
    answered_elsewhere: AnsweredElsewhere {
        at: "22:14",
        where_: "PM pane",
        via: "pm-pane",
        option: 0,
    },
};

/// A file excerpt or log, with source-prescribed illustrative attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artifact {
    /// File title above the content.
    pub file: &'static str,
    /// Lines in source order.
    pub lines: &'static [&'static str],
    /// Whether the content is illustrative rather than the real artifact.
    pub illustrative: bool,
}

/// The real HostQueueObserver interface excerpt from the same patch as the hunk.
pub const EXCERPT: Artifact = Artifact {
    file: "HostQueueObserver (same patch, lines 18–24)",
    illustrative: false,
    lines: &[
        "export interface HostQueueObserver {",
        "    input(input: { text; images?; source: InputSource; mode }): unknown;",
        "    lifecycle(event: { tag; event: PromptDisposition | \"rejected\" }): void;",
        "    queue(change: AgentQueueChange & { entries }): void;",
        "    /** Original pane text when native restoration is allowed;",
        "        undefined keeps the item queued. */",
        "    restoreInput?(tag: unknown): string | undefined;",
        "}",
    ],
};

/// Illustrative tail under a real artifact title.
pub const LOGTAIL: Artifact = Artifact {
    file: "artifacts/T-868/R1/9a87439d-plain-restore-check.log",
    illustrative: true,
    lines: &[
        "plain-restore 52x24   Alt+Up  restored 1 pane item   kept 2 external",
        "plain-restore 52x24   Esc     restored 1 pane item   kept 2 external",
        "plain-restore 140x46  Alt+Up  restored 1 pane item   kept 2 external",
        "plain-restore 140x46  Esc     draft preserved        kept 2 external",
        "external identities unchanged: q-g13-5c1e…-0007, q-g13-5c1e…-0008",
        "PASS  4/4",
    ],
};

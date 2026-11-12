//! Review's replay fixtures. The production patch is read from the shared board hunk.

use crate::model::board::{LineKind, Mark};
use crate::model::hue::Hue;

/// A row in the desktop candidate list. Only T-868 has a replay detail in the mockup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// Task id shown with the candidate SHA.
    pub task: &'static str,
    /// Short title below the task id.
    pub title: &'static str,
    /// Candidate identity to which verdicts apply.
    pub sha: &'static str,
    /// State line below the title.
    pub state: &'static str,
    /// Actor mark colour, independent of selection.
    pub hue: Hue,
}

/// Candidate rows in their replay order.
pub const CANDIDATES: &[Candidate] = &[
    Candidate {
        task: "T-868",
        title: "host-queue-ledger",
        sha: "85b45e1e",
        state: "gate ✓ · review ✓ · you",
        hue: Hue::You,
    },
    Candidate {
        task: "T-890",
        title: "pm-bounded-read-contract",
        sha: "1751bd2",
        state: "quick gate running",
        hue: Hue::Corral,
    },
    Candidate {
        task: "T-894",
        title: "api-connections",
        sha: "6b36916",
        state: "gate running",
        hue: Hue::Corral,
    },
    Candidate {
        task: "T-867",
        title: "queue-contract-core",
        sha: "5949a4b",
        state: "gate stale · re-gate",
        hue: Hue::Held,
    },
];

/// An original verdict record, retaining its original result even when stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Who supplied the verdict.
    pub who: &'static str,
    /// Original result; stale records are displayed with Mark::Stale instead.
    pub mark: Mark,
    /// Candidate on which this result was recorded.
    pub sha: &'static str,
    /// Clock time, or empty for the owner's pending verdict.
    pub time: &'static str,
    /// Result sentence, struck when stale.
    pub text: &'static str,
    /// Whether a newer candidate has superseded this result.
    pub stale: bool,
}

/// Candidate-bound verdicts, including the old failing gate.
pub const VERDICTS: &[Verdict] = &[
    Verdict {
        who: "gate",
        mark: Mark::Fail,
        sha: "f34f3df6",
        time: "04:45",
        text: "fail · cross-lane paging fixture overflow (256 inputs → 528 events, 512 ring)",
        stale: true,
    },
    Verdict {
        who: "gate",
        mark: Mark::Done,
        sha: "85b45e1e",
        time: "08:26",
        text: "pass · quick tier · receipt 77b8a3c1",
        stale: false,
    },
    Verdict {
        who: "review",
        mark: Mark::Done,
        sha: "85b45e1e",
        time: "22:40",
        text: "approve · R1-review2 · notes c4328f8e",
        stale: false,
    },
    Verdict {
        who: "you",
        mark: Mark::Next,
        sha: "85b45e1e",
        time: "",
        text: "your verdict",
        stale: false,
    },
];

/// One source line before numbering and word highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLine {
    /// Context, deletion or addition.
    pub kind: LineKind,
    /// Code without the diff sign.
    pub text: &'static str,
    /// First occurrence of this word receives the word-level highlight.
    pub word: Option<&'static str>,
}

/// An inline review comment and its attachment in the source line sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    /// Zero-based source line after which unified mode inserts the comment.
    pub after: usize,
    /// Reviewer and profile shown in bold.
    pub who: &'static str,
    /// Review round shown alongside the reviewer.
    pub at: &'static str,
    /// Candidate SHA shown after `on`.
    pub sha: &'static str,
    /// Comment body.
    pub text: &'static str,
}

/// The lines and header supplied by a file. Collapsed files have no supplied body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkSource {
    /// Illustrative lines with independent old and new starting counters.
    Inline {
        /// Hunk heading, preserved verbatim.
        header: &'static str,
        /// First old and new line numbers.
        start: (u32, u32),
        /// Lines in source order.
        lines: &'static [SourceLine],
    },
    /// The real `board::hunk()`; Review starts both displayed counters at 1990.
    Board,
    /// The mockup only supplies a collapsed changed-line count.
    Collapsed,
}

/// A file heading, its hunk source and its optional inline discussion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    /// Full path in the file heading.
    pub path: &'static str,
    /// Added count displayed with `+`.
    pub added: u32,
    /// Deleted count displayed with `−`.
    pub deleted: u32,
    /// Header and code source, or the collapsed-body marker.
    pub hunk: HunkSource,
    /// Review thread inserted into this file's diff.
    pub comment: Option<Comment>,
    /// Whether the file's code lines are illustrative.
    pub illustrative: bool,
}

const CROSS_LANE_LINES: &[SourceLine] = &[
    SourceLine {
        kind: LineKind::Context,
        text: "  const lane = await privateHerdr({ plugins: false });",
        word: None,
    },
    SourceLine {
        kind: LineKind::Del,
        text: "  const inputs = 256;",
        word: Some("256"),
    },
    SourceLine {
        kind: LineKind::Add,
        text: "  const inputs = 128;",
        word: Some("128"),
    },
    SourceLine {
        kind: LineKind::Context,
        text: "  for (let i = 0; i < inputs; i++) await submit(lane, `msg ${i}`);",
        word: None,
    },
    SourceLine {
        kind: LineKind::Context,
        text: "  const events = await readAll(lane, { page: 64 });",
        word: None,
    },
    SourceLine {
        kind: LineKind::Context,
        text: "  expect(events.length).toBeGreaterThan(256);",
        word: None,
    },
];

/// The five files shown on the Diff tab, in source order.
pub const FILES: &[File] = &[
    File {
        path: "packages/corral-pi/test/cross-lane.test.ts",
        added: 1,
        deleted: 1,
        hunk: HunkSource::Inline {
            header: "@@ -212,9 +212,9 @@ describe(\"cross-lane paging\")",
            start: (212, 212),
            lines: CROSS_LANE_LINES,
        },
        comment: Some(Comment {
            after: 2,
            who: "reviewer · pi-sol",
            at: "R1-review2",
            sha: "85b45e1e",
            text: "Fixture-only change: 256 → 128 inputs. Every assertion, the 512 ring and the production event/frame bounds are unchanged; the original red on f34f3df6 stays in history.",
        }),
        illustrative: true,
    },
    File {
        path: "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch",
        added: 9,
        deleted: 4,
        hunk: HunkSource::Board,
        comment: None,
        illustrative: false,
    },
    File {
        path: "packages/corral-pi/src/host/queue.ts",
        added: 212,
        deleted: 18,
        hunk: HunkSource::Collapsed,
        comment: None,
        illustrative: false,
    },
    File {
        path: "packages/corral-pi/src/bridge/queue.ts",
        added: 88,
        deleted: 3,
        hunk: HunkSource::Collapsed,
        comment: None,
        illustrative: false,
    },
    File {
        path: "docs/guide/pi-native.md",
        added: 24,
        deleted: 2,
        hunk: HunkSource::Collapsed,
        comment: None,
        illustrative: false,
    },
];

/// A marked acceptance criterion in the rendered implementation report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acceptance {
    /// Pass mark displayed before the criterion.
    pub mark: Mark,
    /// Criterion text.
    pub text: &'static str,
}

/// The report tab's document, including its real artifact provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Document heading.
    pub heading: &'static str,
    /// Paragraphs before the acceptance list.
    pub paragraphs: &'static [&'static str],
    /// Acceptance list heading.
    pub acceptance_heading: &'static str,
    /// Ordered marked criteria.
    pub acceptance: &'static [Acceptance],
    /// Artifact path shown in monospace after `Source:`.
    pub source: &'static str,
    /// Provenance suffix following the artifact path.
    pub source_note: &'static str,
}

/// Rendered, condensed report from T-868 R1.
pub const REPORT: Report = Report {
    heading: "T-868 implementation report · rendered",
    paragraphs: &[
        "Implemented the Pi 1.5.0 host queue ledger with stable bound identities, synchronous pre-mutation lifecycle persistence, bounded bridge inspection/withdrawal, canonical operation replay and pane-only interactive edits. Delivery observation remains separate from execution and turn completion.",
    ],
    acceptance_heading: "Acceptance",
    acceptance: &[
        Acceptance {
            mark: Mark::Done,
            text: "Targeted withdrawal and both start races with exact-event barriers",
        },
        Acceptance {
            mark: Mark::Done,
            text: "Replay, conflict, operation bound, foreign/stale targets leave the queue unchanged",
        },
        Acceptance {
            mark: Mark::Done,
            text: "Pane edit, persistence, reload, rich Esc/dequeue and plain restore",
        },
        Acceptance {
            mark: Mark::Done,
            text: "Package/host/bridge/composer checks on Pi 1.5.0; compat lock hash agrees",
        },
    ],
    source: "artifacts/T-868/R1/1c6d848a-T-868-implementation-report.md",
    source_note: " and run result (real text, condensed)",
};

/// One preformatted gate-log line with its success colour resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLine {
    /// The complete line, including indentation.
    pub text: &'static str,
    /// Only lines containing `pass` or `PASS` use Ok.
    pub hue: Hue,
}

/// Gate-log lines; the receipt id is real and the log text is illustrative.
pub const GATE_LOG: &[LogLine] = &[
    LogLine {
        text: "$ scripts/gate.sh --tier quick  (85b45e1e)",
        hue: Hue::Neutral,
    },
    LogLine {
        text: "  build     corral-pi            12.4s",
        hue: Hue::Neutral,
    },
    LogLine {
        text: "  bun test  queue/bridge         16 pass",
        hue: Hue::Ok,
    },
    LogLine {
        text: "  cross-lane private herdr        5 pass",
        hue: Hue::Ok,
    },
    LogLine {
        text: "  receipt   77b8a3c1",
        hue: Hue::Neutral,
    },
    LogLine {
        text: "PASS",
        hue: Hue::Ok,
    },
];

/// The provenance label under the gate log.
pub const GATE_LOG_NOTE: &str = "log lines illustrative · receipt id real";

/// The screenshot tab's real artifact title and illustrative terminal content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screenshot {
    /// Session heading inside the terminal capture.
    pub heading: &'static str,
    /// Plain terminal contents, including blank lines and indentation.
    pub text: &'static str,
    /// Real ANSI artifact path.
    pub source: &'static str,
    /// Caption shown after the artifact path.
    pub description: &'static str,
}

/// Screenshot evidence; the title is real, not the illustrated terminal contents.
pub const SCREENSHOT: Screenshot = Screenshot {
    heading: "pi · bound session · plain mode · 140x46",
    text: "pi · bound session · plain mode · 140x46\n\n› draft kept: \"tighten the withdraw race test\"\n\nEsc  aborted turn · restored 1 pane item\n     external identities retained: 2\n     queued  1 steering · 1 follow-up\n\n› tighten the withdraw race test",
    source: "artifacts/T-868/R1/838f50e2-140x46-esc.ansi",
    description: "\"140x46 real plain-mode terminal after Esc abort with external items retained\" (title real, content illustrative)",
};

//! Plan revisions, review documents and the deterministic synthetic tracker.

use crate::model::board::Mark;

/// Which Plans section holds a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanState {
    /// The owner reviews a revision before publishing members.
    Awaiting,
    /// Published members are progressing through delivery.
    Executing,
    /// Approved, but no tracker or members were published.
    Approved,
}

/// One proposed member name and the names it follows on publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proposal {
    /// The proposed task name, not a published task id.
    pub name: &'static str,
    /// Proposed prerequisites, in document order.
    pub needs: &'static [&'static str],
}

/// One reviewer verdict, bound to its original revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Review {
    /// Revision the reviewer saw.
    pub rev: &'static str,
    /// Original verdict mark; superseded revisions are displayed as stale.
    pub mark: Mark,
    /// The verdict word.
    pub verdict: &'static str,
    /// Reviewer profile.
    pub who: &'static str,
    /// Review completion time.
    pub at: &'static str,
    /// The reviewer's summary.
    pub note: &'static str,
}

/// The complete awaiting-acceptance document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Document {
    /// Problem paragraph.
    pub problem: &'static str,
    /// Goal paragraph.
    pub goal: &'static str,
    /// Changes bullets.
    pub changes: &'static [&'static str],
    /// Keeps bullets.
    pub retained: &'static [&'static str],
    /// Not doing bullets.
    pub non_goals: &'static [&'static str],
    /// Proposed members published only after acceptance.
    pub proposed: &'static [Proposal],
    /// Revision-bound reviewer verdicts.
    pub reviews: &'static [Review],
    /// Artifact path and size.
    pub artifact: &'static str,
}

/// Every field of a plan snapshot; absent data remains absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Plan or tracker id.
    pub id: &'static str,
    /// Document revision, when this is a reviewable plan.
    pub rev: Option<&'static str>,
    /// Full title.
    pub title: &'static str,
    /// List section and detail variant.
    pub state: PlanState,
    /// Delivery branch, when explicitly recorded.
    pub delivery: Option<&'static str>,
    /// Published task ids or proposed member names, in original order.
    pub members: &'static [&'static str],
    /// Dependency layers; empty means one layer containing all members.
    pub layers: &'static [&'static [&'static str]],
    /// Acceptance attribution.
    pub accepted: Option<&'static str>,
    /// Acceptance scenario count, when recorded.
    pub scenarios: Option<u32>,
    /// Mapped scenario count, when recorded.
    pub mapped: Option<u32>,
    /// Final-acceptance blockers in display order.
    pub blockers: &'static [&'static str],
    /// Approved-but-unpublished note.
    pub note: Option<&'static str>,
    /// The awaiting revision's review document.
    pub document: Option<&'static Document>,
}

/// The original P2 review document, including superseded P1 evidence.
pub static DOCUMENT: Document = Document {
    problem: "The PM owner and agent need a bounded and consistent view of current responsibilities; operational continuity should not depend on an ever-growing conversation.",
    goal: "Deliver a bounded first wave of PM information and lifecycle visibility improvements, while reviewing a longer-term hybrid supervisor design without authorizing it wholesale.",
    changes: &[
        "A bounded common semantic read contract for PM CLI and native tools, with scope-specific rendering and complete detail access.",
        "Existing attention, inbox, question, wait and receipt records as one read-only obligation view; no second scheduler or task database.",
        "Measure the improved loop and document an evidence-gated warm-context episode pilot for later review.",
    ],
    retained: &[
        "One logical PM, current conversation lifetime, existing sync and wake ownership, current deterministic executors.",
        "Current scope checks, owner authority, receipts, candidate identities, gates, independent review and landing checks.",
    ],
    non_goals: &[
        "No fresh model session per event or mandatory conversation rollover.",
        "No new daemon, scheduler, obligation database, memory engine or manager hierarchy.",
        "No provider/model change, paid evaluation, recurring model job or background reflection.",
    ],
    proposed: &[
        Proposal {
            name: "pm-bounded-read-contract",
            needs: &[],
        },
        Proposal {
            name: "pm-obligation-read-view",
            needs: &["pm-bounded-read-contract"],
        },
        Proposal {
            name: "pm-read-costs-and-guidance",
            needs: &["pm-obligation-read-view"],
        },
    ],
    reviews: &[
        Review {
            rev: "P1",
            mark: Mark::Fail,
            verdict: "changes",
            who: "pi-sol",
            at: "10:33",
            note: "Direction sound; two read-contract decisions (F1 pagination/fairness, F2) unresolved.",
        },
        Review {
            rev: "P2",
            mark: Mark::Done,
            verdict: "approve",
            who: "pi-sol",
            at: "12:10",
            note: "F1 and F2 addressed. No blocking defect. Approval is for owner consideration only.",
        },
    ],
    artifact: "artifacts/T-885/R0/b494fef5-pm-supervisor-plan-v2.md · 43.9 KB",
};

/// The six plans in the mockup's navigation order.
pub static PLANS: &[Plan] = &[
    Plan {
        id: "T-885",
        rev: Some("P2"),
        state: PlanState::Awaiting,
        title: "PM supervisor evolution: bounded reads, warm-context episodes",
        delivery: Some("main"),
        members: &[
            "pm-bounded-read-contract",
            "pm-obligation-read-view",
            "pm-read-costs-and-guidance",
        ],
        layers: &[],
        accepted: None,
        scenarios: Some(6),
        mapped: None,
        blockers: &[],
        note: None,
        document: Some(&DOCUMENT),
    },
    Plan {
        id: "T-870",
        rev: None,
        state: PlanState::Executing,
        title: "Pi input: stable queue item lifecycle, editing, cancellation",
        delivery: None,
        members: &["T-867", "T-868", "T-869"],
        layers: &[&["T-867"], &["T-868", "T-869"]],
        accepted: Some("earlier"),
        scenarios: Some(15),
        mapped: Some(0),
        blockers: &[
            "T-867 required work is Running",
            "T-868 waits on your answer",
            "T-869 required work is Ready",
        ],
        note: None,
        document: None,
    },
    Plan {
        id: "T-893",
        rev: None,
        state: PlanState::Executing,
        title: "PM supervisor evolution: bounded reads, warm-context episodes",
        delivery: None,
        members: &["T-890", "T-891", "T-892"],
        layers: &[&["T-890"], &["T-891"], &["T-892"]],
        accepted: Some("you 19:12"),
        scenarios: Some(6),
        mapped: Some(0),
        blockers: &[
            "T-890 required work is Running",
            "T-891 required work is Ready",
            "T-892 required work is Ready",
        ],
        note: None,
        document: None,
    },
    Plan {
        id: "T-898",
        rev: None,
        state: PlanState::Executing,
        title: "Anthropic + OpenAI API connections, bounded subscription fallback",
        delivery: None,
        members: &["T-894", "T-895", "T-896", "T-897"],
        layers: &[&["T-894"], &["T-895", "T-896"], &["T-897"]],
        accepted: Some("earlier"),
        scenarios: Some(7),
        mapped: Some(0),
        blockers: &[
            "T-894 required work is Running",
            "T-895 required work is Ready",
            "T-896 required work is Ready",
            "T-897 required work is Ready",
        ],
        note: None,
        document: None,
    },
    Plan {
        id: "T-874",
        rev: None,
        state: PlanState::Executing,
        title: "Pi review annotations: durable candidate-bound threads",
        delivery: None,
        members: &["T-871", "T-872", "T-873"],
        layers: &[&["T-871"], &["T-872", "T-873"]],
        accepted: Some("earlier"),
        scenarios: Some(12),
        mapped: Some(0),
        blockers: &[
            "T-871 required work is Running",
            "T-872 required work is Ready",
            "T-873 required work is Ready",
        ],
        note: None,
        document: None,
    },
    Plan {
        id: "T-385",
        rev: Some("P5"),
        state: PlanState::Approved,
        title: "Design: reliable task progress and PM attention across harnesses",
        delivery: None,
        members: &[],
        layers: &[],
        accepted: None,
        scenarios: None,
        mapped: None,
        blockers: &[],
        note: Some("4 approvals · no tracker published"),
        document: None,
    },
];

/// Find the selected plan, using T-870 for an unknown id like the mockup.
pub fn plan(id: &str) -> &'static Plan {
    PLANS.iter().find(|plan| plan.id == id).unwrap_or(&PLANS[1])
}

/// One square's state in the synthetic layer strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleState {
    /// Delivery complete; no expanded line.
    Landed,
    /// Gating, with an expanded line.
    Active,
    /// Owner action required, with an expanded line.
    You,
    /// Budget hold, with an expanded line.
    Held,
    /// Waiting on the preceding layer; no expanded line.
    Waiting,
}

/// A generated member; `name()` joins the generated stem and number for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleMember {
    /// Static synthetic task id.
    pub id: &'static str,
    /// Zero-based layer index.
    pub layer: usize,
    /// Generated name prefix.
    pub stem: &'static str,
    /// Generated name suffix.
    pub suffix: usize,
    /// The square and expanded-line state.
    pub state: ScaleState,
}

impl ScaleMember {
    /// The exact synthetic name shown next to the id.
    pub fn name(self) -> String {
        format!("{}-{}", self.stem, self.suffix)
    }
}

/// The six original `[size, active, needs-you]` generator inputs.
pub const SCALE_LAYERS: [[usize; 3]; 6] = [
    [14, 0, 0],
    [12, 1, 1],
    [10, 2, 1],
    [11, 1, 0],
    [9, 0, 0],
    [8, 0, 0],
];

const SCALE_IDS: [&str; 64] = [
    "S-901", "S-902", "S-903", "S-904", "S-905", "S-906", "S-907", "S-908", "S-909", "S-910",
    "S-911", "S-912", "S-913", "S-914", "S-915", "S-916", "S-917", "S-918", "S-919", "S-920",
    "S-921", "S-922", "S-923", "S-924", "S-925", "S-926", "S-927", "S-928", "S-929", "S-930",
    "S-931", "S-932", "S-933", "S-934", "S-935", "S-936", "S-937", "S-938", "S-939", "S-940",
    "S-941", "S-942", "S-943", "S-944", "S-945", "S-946", "S-947", "S-948", "S-949", "S-950",
    "S-951", "S-952", "S-953", "S-954", "S-955", "S-956", "S-957", "S-958", "S-959", "S-960",
    "S-961", "S-962", "S-963", "S-964",
];

const fn generate_scale() -> [ScaleMember; 64] {
    let names = [
        "schema", "codec", "index", "cursor", "pager", "ledger", "replay", "fence", "lease",
        "probe", "bridge", "relay", "sweep", "audit", "quota", "notice",
    ];
    let mut out = [ScaleMember {
        id: "",
        layer: 0,
        stem: "",
        suffix: 0,
        state: ScaleState::Waiting,
    }; 64];
    let mut n = 0;
    let mut li = 0;
    while li < SCALE_LAYERS.len() {
        let [size, active, you] = SCALE_LAYERS[li];
        let mut i = 0;
        while i < size {
            let mut state = if li == 0 || (li == 1 && i >= 5 + active + you) {
                ScaleState::Landed
            } else {
                ScaleState::Waiting
            };
            if li == 1 && i < 5 {
                state = ScaleState::Landed;
            }
            if (li == 1 && i == 5) || (li == 2 && i < 2) {
                state = ScaleState::Active;
            }
            if (li == 1 && i == 6) || (li == 2 && i == 2) {
                state = ScaleState::You;
            }
            if li == 3 && i == 0 {
                state = ScaleState::Held;
            }
            out[n] = ScaleMember {
                id: SCALE_IDS[n],
                layer: li,
                stem: names[((n + 1) * 7) % names.len()],
                suffix: ((n + 1) * 13) % 97,
                state,
            };
            n += 1;
            i += 1;
        }
        li += 1;
    }
    out
}

/// All 64 synthetic members, generated at compile time in layer and id order.
pub const SCALE: [ScaleMember; 64] = generate_scale();

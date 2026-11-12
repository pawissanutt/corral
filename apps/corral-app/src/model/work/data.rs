//! Who-acts-next groups and owner dispatch holds from the board snapshot.

use crate::model::hue::Hue;

/// The six who-acts-next sections, in their displayed order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKey {
    /// Questions, plan accepts and PM asks.
    You,
    /// Gates, reviews and landing.
    Corral,
    /// Producing worker runs.
    Worker,
    /// Waiting for dependency or capacity.
    Queued,
    /// Held by an external operation.
    Held,
    /// Owner-set dispatch holds, not counted as Needs you.
    Parked,
}

impl GroupKey {
    /// The state hue of the group's heading and actor mark.
    pub const fn hue(self) -> Hue {
        match self {
            Self::You => Hue::You,
            Self::Corral => Hue::Corral,
            Self::Worker => Hue::Worker,
            Self::Held => Hue::Held,
            Self::Queued | Self::Parked => Hue::Neutral,
        }
    }

    /// The mockup's group key, useful for stable row identity.
    pub const fn key(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Corral => "corral",
            Self::Worker => "worker",
            Self::Queued => "queued",
            Self::Held => "held",
            Self::Parked => "parked",
        }
    }
}

/// A non-task row in the Needs you group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraKind {
    /// A reviewed plan awaiting acceptance.
    Plan,
    /// An illustrative PM question.
    Ask,
}

/// The plan/PM rows that accompany the needs-you task on desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extra {
    /// Whether to show the plan or PM-ask row anatomy.
    pub kind: ExtraKind,
    /// Full fixture id; display removes the `PM/` prefix.
    pub id: &'static str,
    /// Row title, kept verbatim.
    pub title: &'static str,
    /// The now-phrase to the right or on the second phone line.
    pub note: &'static str,
    /// The PM ask is sample content, not a real question.
    pub illustrative: bool,
}

/// The owner-held summary that replaces 32 individual parked rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkedSummary {
    /// Total owner-held tasks.
    pub n: u32,
    /// Counts by project, in the source's wording.
    pub split: &'static str,
    /// Explanation of why these tasks do not need an answer.
    pub note: &'static str,
}

/// A group of tasks plus optional non-task and collapsed summary rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    /// Stable section identity and state hue.
    pub key: GroupKey,
    /// Visible section heading.
    pub label: &'static str,
    /// Task ids, in visible list order.
    pub ids: &'static [&'static str],
    /// Additional plan and PM-ask rows.
    pub extra: &'static [Extra],
    /// Additional queued tasks hidden on desktop.
    pub more: u32,
    /// The parked aggregate, when this is the parked section.
    pub parked: Option<ParkedSummary>,
}

/// Board groups, including the illustrative PM ask and all eight visible queued ids.
pub const GROUPS: &[Group] = &[
    Group {
        key: GroupKey::You,
        label: "Needs you",
        ids: &["T-868"],
        extra: &[
            Extra {
                kind: ExtraKind::Plan,
                id: "T-885",
                title: "plan P2 · accept?",
                note: "reviewed · 3 members",
                illustrative: false,
            },
            Extra {
                kind: ExtraKind::Ask,
                id: "PM/Q7",
                title: "T-852 repair budget: re-gate only, or one more round?",
                note: "12m · sample",
                illustrative: true,
            },
        ],
        more: 0,
        parked: None,
    },
    Group {
        key: GroupKey::Corral,
        label: "Corral is verifying",
        ids: &["T-890", "T-894", "T-867"],
        extra: &[],
        more: 0,
        parked: None,
    },
    Group {
        key: GroupKey::Worker,
        label: "Workers producing",
        ids: &["T-871", "T-854"],
        extra: &[],
        more: 0,
        parked: None,
    },
    Group {
        key: GroupKey::Queued,
        label: "Queued",
        ids: &[
            "T-891", "T-892", "T-895", "T-896", "T-897", "T-869", "T-872", "T-873",
        ],
        extra: &[],
        more: 15,
        parked: None,
    },
    Group {
        key: GroupKey::Held,
        label: "Held",
        ids: &["T-852"],
        extra: &[],
        more: 0,
        parked: None,
    },
    Group {
        key: GroupKey::Parked,
        label: "Parked",
        ids: &[],
        extra: &[],
        more: 0,
        parked: Some(ParkedSummary {
            n: 32,
            split: "25 corral · 5 corral-ext-mitigraph · 2 mitimind-lite",
            note: "Each has a dispatch hold you set. They wait here, not under Needs you.",
        }),
    },
];

/// An individual parked task in the lower-left holds table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold {
    /// Mono task id.
    pub id: &'static str,
    /// Project shown dimmed after the task title.
    pub project: &'static str,
    /// Full held task title.
    pub title: &'static str,
    /// Date the hold was placed, in MM-DD form.
    pub since: &'static str,
    /// The recorded owner answer, where the fixture has one.
    pub note: Option<&'static str>,
}

/// The six representative rows of the Parked table.
pub const HOLDS: &[Hold] = &[
    Hold {
        id: "T-130",
        project: "mitimind-lite",
        title: "Knowledge engine 31: verify owner-approved merged delivery",
        since: "11-05",
        note: Some("You answered 11-07 08:12: keep held; finish Corral first"),
    },
    Hold {
        id: "T-157",
        project: "corral",
        title: "PRF-05: first-run and profile-selection TUI flows",
        since: "11-05",
        note: None,
    },
    Hold {
        id: "T-158",
        project: "corral",
        title: "PRF-06: mapping refresh and explicit legacy migration",
        since: "11-05",
        note: None,
    },
    Hold {
        id: "T-187",
        project: "corral",
        title: "Cost and performance analytics",
        since: "10-20",
        note: None,
    },
    Hold {
        id: "T-194",
        project: "corral",
        title: "Board glance: spend summary and report shortcut",
        since: "11-05",
        note: None,
    },
    Hold {
        id: "T-209",
        project: "corral",
        title: "Agent inbox 4/5: omp corral-inbox extension adapter",
        since: "11-05",
        note: None,
    },
];

/// Extra terminal lines supplied by the worker snapshot when T-868 has no board tail.
pub const QUESTION_TAIL: &[&str] = &[
    "Edit  packages/corral-pi/src/host/queue.ts",
    "      +41 -6",
    "Ask   T-868/R1/Q1 restore policy",
    "Bash  bun test test/queue-host.test.ts",
    "      16 pass",
];

/// One live-context tile's snapshot metadata; task facts still come from `board::task`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    /// Agent identity, distinct from a gate's task id.
    pub id: &'static str,
    /// Task opened by the tile, or PM for the supervisor.
    pub session: &'static str,
    /// Agent role label.
    pub role: &'static str,
    /// Human-readable runtime state.
    pub state: &'static str,
    /// The current action line.
    pub doing: &'static str,
    /// Pane identity.
    pub pane: &'static str,
    /// Profile or gate script.
    pub profile: &'static str,
    /// Runtime name; the legacy runtime only supplies snapshots.
    pub runtime: &'static str,
    /// Clock when this agent state began.
    pub since: &'static str,
    /// State colour, not selection colour.
    pub hue: Hue,
}

/// What runs now, in the source's PM/worker/gate order.
pub const RUNNING: &[Running] = &[
    Running {
        id: "PM",
        session: "PM",
        role: "PM",
        state: "live",
        doing: "waiting · next wake on T-894 gate",
        pane: "pm:p0",
        profile: "pi-sol xhigh",
        runtime: "herdr-core",
        since: "21:40",
        hue: Hue::Corral,
    },
    Running {
        id: "T-871",
        session: "T-871",
        role: "worker",
        state: "working",
        doing: "Edit crates/corral-core/src/annot/record.rs",
        pane: "w14A:p1",
        profile: "pi-sol xhigh",
        runtime: "herdr-core",
        since: "21:17",
        hue: Hue::Worker,
    },
    Running {
        id: "T-854",
        session: "T-854",
        role: "worker",
        state: "working",
        doing: "cargo test -p corral-ops cowork::publish",
        pane: "w15B:p1",
        profile: "pi-sol xhigh",
        runtime: "Herdr TUI (legacy)",
        since: "01:10",
        hue: Hue::Worker,
    },
    Running {
        id: "T-868",
        session: "T-868",
        role: "worker",
        state: "asking",
        doing: "waiting on you · T-868/R1/Q1 · ledger work continues",
        pane: "w12C:p2",
        profile: "pi-sol xhigh",
        runtime: "herdr-core",
        since: "19:12",
        hue: Hue::You,
    },
    Running {
        id: "gate-T-890",
        session: "T-890",
        role: "gate",
        state: "gating",
        doing: "nextest corral-ops 412/980",
        pane: "g:T-890",
        profile: "scripts/gate.sh",
        runtime: "herdr-core",
        since: "22:03",
        hue: Hue::Corral,
    },
    Running {
        id: "gate-T-894",
        session: "T-894",
        role: "gate",
        state: "gating",
        doing: "Compiling corral-tui",
        pane: "g:T-894",
        profile: "scripts/gate.sh",
        runtime: "herdr-core",
        since: "22:06",
        hue: Hue::Corral,
    },
    Running {
        id: "T-867",
        session: "T-867",
        role: "worker",
        state: "idle",
        doing: "idle · candidate 5949a4b awaits re-gate",
        pane: "w12C:p1",
        profile: "pi-sol xhigh",
        runtime: "herdr-core",
        since: "22:05",
        hue: Hue::Neutral,
    },
];

/// The PM's illustrative terminal tail, used only by the running-agent tile.
pub const PM_TAIL: &[&str] = &[
    "wake  sync tick · 3 changes",
    "read  corral pm next --bounded",
    "note  T-852 budget question drafted (PM/Q7)",
    "idle  next wake on gate result",
];

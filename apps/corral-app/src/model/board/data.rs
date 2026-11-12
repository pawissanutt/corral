use super::{Actor, Change, ChangeKind, Hunk, LineKind, Mark, Task};

pub(super) static TASKS: &[Task] = &[
    Task {
        title: "host-queue-ledger",
        plan: Some("T-870"),
        actor: Actor::You,
        rail: [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        now: "needs you · restore policy",
        age: Some("3h"),
        cost: None,
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("18:58"),
        machine: Some("workstation"),
        goal: Some(
            "Give queued Pi-native input stable item identities and a durable lifecycle that keeps delivery acknowledgement (admitted, observed) distinct from execution (queued, started, then turn completion), and let authorized callers inspect, withdraw and edit queued items with defined races against consumption.",
        ),
        needs: &[("T-701", Mark::Done), ("T-867", Mark::Stale)],
        unblocks: &[],
        steps: &[
            (Mark::Done, "Verify Pi 1.5.0 metadata, refresh frozen deps"),
            (Mark::Done, "Stable queue identities + lifecycle journal"),
            (Mark::Active, "Restore policy on stock/plain paths"),
            (Mark::Next, "Bridge queue_state / queue_withdraw"),
            (Mark::Next, "Rebase, quick gate, attach proof"),
        ],
        next: Some(("You", "answer T-868/R1/Q1")),
        pane: Some("w12C:p2"),
        ..Task::unknown("T-868")
    },
    Task {
        title: "pm-bounded-read-contract",
        plan: Some("T-893"),
        actor: Actor::Corral,
        rail: [Mark::Done, Mark::Active, Mark::Next, Mark::Next],
        now: "quick gate 1751bd2",
        age: Some("9m"),
        cost: Some(3.9),
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("19:31"),
        machine: Some("workstation"),
        steps: &[
            (Mark::Done, "Read plan, inspect scope/read contracts"),
            (
                Mark::Done,
                "Shared bounded projection + CLI/native adapters",
            ),
            (Mark::Done, "Verify live paging, byte caps, compatibility"),
            (Mark::Active, "Commit, rebase, gate and attach proof"),
        ],
        next: Some(("Corral", "quick gate finishes → independent review")),
        needs: &[("T-885", Mark::Done)],
        unblocks: &["T-891"],
        pane: Some("w12C:p4"),
        tail: &[
            "$ scripts/gate.sh --tier quick",
            "  build     corral-ops corral-tui   41.2s",
            "  nextest   corral-ops  412/980",
            "    PASS  pm_workset::bounded_page",
            "    PASS  pm_workset::byte_cap_trims",
            "    PASS  pm_next::keyset_live_update",
            "    ....  read_view::scope_paging",
        ],
        ..Task::unknown("T-890")
    },
    Task {
        title: "api-connections",
        plan: Some("T-898"),
        actor: Actor::Corral,
        rail: [Mark::Done, Mark::Active, Mark::Next, Mark::Next],
        now: "gate 6b36916",
        age: Some("6m"),
        cost: Some(18.11),
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("19:33"),
        machine: Some("workstation"),
        steps: &[
            (Mark::Done, "Trace binding, auth, guarded transport"),
            (Mark::Done, "Explicit isolated connections"),
            (Mark::Done, "Focused offline regression checks"),
            (Mark::Active, "Commit, rebase, gate and attach evidence"),
        ],
        next: Some(("Corral", "gate finishes → independent review")),
        needs: &[("T-888", Mark::Done)],
        unblocks: &["T-895", "T-896"],
        pane: Some("gate"),
        tail: &[
            "$ scripts/gate.sh",
            "  Compiling corral-ops",
            "  Compiling corral-pi-host",
            "  Compiling corral-tui",
        ],
        ..Task::unknown("T-894")
    },
    Task {
        title: "queue-contract-core",
        plan: Some("T-870"),
        actor: Actor::Corral,
        rail: [Mark::Done, Mark::Stale, Mark::Next, Mark::Next],
        now: "stale 5949a4b · re-gate",
        age: Some("7m"),
        cost: Some(1.83),
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("20:40"),
        machine: Some("workstation"),
        steps: &[
            (
                Mark::Done,
                "Catalog defaults, negotiation, immutable history",
            ),
            (Mark::Done, "Monotonic execution records, pending indexes"),
            (Mark::Done, "Immutable intents/outcomes, atomic edits"),
            (
                Mark::Active,
                "Targeted checks, evidence, rebase, quick gate",
            ),
        ],
        next: Some(("Corral", "re-gate on the rebased candidate")),
        needs: &[("T-701", Mark::Done)],
        unblocks: &["T-868", "T-869"],
        pane: Some("worker"),
        tail: &[
            "worker idle · candidate 5949a4b",
            "gate result: stale (base moved)",
        ],
        ..Task::unknown("T-867")
    },
    Task {
        title: "Co-work C1: shared completion engine + failure triage",
        plan: Some("T-851"),
        actor: Actor::Held,
        rail: [Mark::Done, Mark::Stale, Mark::Next, Mark::Next],
        now: "held · external op",
        age: Some("2h44"),
        cost: Some(43.4),
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("09:43"),
        machine: Some("workstation"),
        round: Some(2),
        steps: &[
            (Mark::Done, "Trace design and existing pipeline"),
            (Mark::Done, "Durable async submit, current-driver"),
            (Mark::Done, "Focused regression checks"),
            (Mark::Done, "Commit, rebase, receipted quick gate"),
            (Mark::Done, "Reproduce + fix infra episode (round 2)"),
            (Mark::Done, "Validate, rebase, fresh quick gate"),
        ],
        next: Some((
            "Corral",
            "external operation must finish (2h44m), then re-gate",
        )),
        needs: &[("T-802", Mark::Done)],
        unblocks: &["T-851"],
        pane: Some("worker"),
        tail: &["worker done · round 2", "gate ◌ stale af7a6dc"],
        ..Task::unknown("T-852")
    },
    Task {
        title: "annotation-service",
        plan: Some("T-874"),
        actor: Actor::Worker,
        rail: [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        now: "working · step 1/3",
        age: Some("55m"),
        cost: Some(3.29),
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("21:17"),
        machine: Some("workstation"),
        steps: &[
            (
                Mark::Active,
                "Versioned records + canonical indexed persistence",
            ),
            (
                Mark::Next,
                "Git anchors, authorized mutations, bounded reads",
            ),
            (Mark::Next, "Targeted acceptance tests and quick gate"),
        ],
        next: Some(("Worker", "finish step 1 of 3")),
        needs: &[("T-699", Mark::Done)],
        unblocks: &["T-872", "T-873"],
        pane: Some("w14A:p1"),
        tail: &[
            "Edit  crates/corral-core/src/annot/record.rs",
            "      +118 -4",
            "Bash  cargo check -p corral-core",
            "      ok 14.8s",
        ],
        ..Task::unknown("T-871")
    },
    Task {
        title: "Co-work C3: save, take over, take back",
        plan: Some("T-851"),
        actor: Actor::Worker,
        rail: [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        now: "working · publication",
        age: Some("1h"),
        cost: Some(7.56),
        changed: true,
        agent: Some("pi-sol xhigh"),
        run: Some("R1"),
        started: Some("01:10"),
        machine: Some("workstation"),
        steps: &[
            (
                Mark::Done,
                "Read design, inspect ownership journal and admission seams",
            ),
            (
                Mark::Active,
                "Held task publication, journal recovery, scoped intent",
            ),
            (Mark::Next, "Stage-aware adoption and plan-only stop guards"),
            (Mark::Next, "Settled take-back and landing serialization"),
            (
                Mark::Next,
                "Targeted lifecycle regressions and criterion evidence",
            ),
            (Mark::Next, "Commit, rebase, receipted quick gate"),
        ],
        next: Some(("Worker", "finish the publication step")),
        needs: &[("T-852", Mark::Done), ("T-853", Mark::Done)],
        unblocks: &["T-855"],
        pane: Some("w15B:p1"),
        tail: &[
            "Read  artifacts/T-854/R1/edf0576e-c3-checkpoint-notes.md",
            "Edit  crates/corral-ops/src/cowork/publish.rs",
            "      +64 -12",
            "Bash  cargo test -p corral-ops cowork::publish",
            "      running 9 tests",
        ],
        ..Task::unknown("T-854")
    },
    Task {
        title: "pm-obligation-read-view",
        plan: Some("T-893"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-890",
        needs: &[("T-885", Mark::Done), ("T-890", Mark::Active)],
        unblocks: &["T-892"],
        ..Task::unknown("T-891")
    },
    Task {
        title: "pm-read-costs-and-guidance",
        plan: Some("T-893"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-891",
        needs: &[("T-891", Mark::Next)],
        unblocks: &[],
        ..Task::unknown("T-892")
    },
    Task {
        title: "failure-signals",
        plan: Some("T-898"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-894",
        needs: &[("T-894", Mark::Active)],
        unblocks: &[],
        ..Task::unknown("T-895")
    },
    Task {
        title: "metered-fallback",
        plan: Some("T-898"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-894",
        needs: &[("T-894", Mark::Active)],
        unblocks: &["T-897"],
        ..Task::unknown("T-896")
    },
    Task {
        title: "role-continuations",
        plan: Some("T-898"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-896",
        needs: &[("T-896", Mark::Next)],
        unblocks: &[],
        ..Task::unknown("T-897")
    },
    Task {
        title: "inbox-queue-ops",
        plan: Some("T-870"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-867",
        needs: &[("T-867", Mark::Stale)],
        unblocks: &[],
        ..Task::unknown("T-869")
    },
    Task {
        title: "annotation-owner-cli",
        plan: Some("T-874"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-871",
        needs: &[("T-871", Mark::Active)],
        unblocks: &[],
        ..Task::unknown("T-872")
    },
    Task {
        title: "annotation-pi-host-api",
        plan: Some("T-874"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-871",
        needs: &[("T-871", Mark::Active)],
        unblocks: &[],
        ..Task::unknown("T-873")
    },
    Task {
        title: "Co-work C4: assistant + board handoff TUI",
        plan: Some("T-851"),
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "after T-854",
        ..Task::unknown("T-855")
    },
    Task {
        title: "Record reviewer child outcome for co-work resubmit",
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "queued · capacity",
        ..Task::unknown("T-883")
    },
    Task {
        title: "Co-work finish missing usage after diag failure",
        actor: Actor::Queued,
        rail: [Mark::Next, Mark::Next, Mark::Next, Mark::Next],
        now: "queued · capacity",
        ..Task::unknown("T-832")
    },
    Task {
        title: "Pi input queue lifecycle design",
        actor: Actor::Done,
        rail: [Mark::Na, Mark::Na, Mark::Na, Mark::Na],
        now: "plan P2 approved",
        ..Task::unknown("T-701")
    },
    Task {
        title: "PM supervisor evolution: bounded reads",
        actor: Actor::Done,
        rail: [Mark::Na, Mark::Na, Mark::Na, Mark::Na],
        now: "plan P2 accepted",
        ..Task::unknown("T-885")
    },
];

pub(super) static CHANGES: &[Change] = &[
    Change {
        at: "22:06",
        id: "T-894",
        kind: ChangeKind::Mark(Mark::Active),
        text: "gate started on 6b36916",
    },
    Change {
        at: "22:05",
        id: "T-867",
        kind: ChangeKind::Mark(Mark::Stale),
        text: "gate went stale on 5949a4b (base moved)",
    },
    Change {
        at: "22:03",
        id: "T-890",
        kind: ChangeKind::Mark(Mark::Active),
        text: "quick gate started on 1751bd2",
    },
    Change {
        at: "22:03",
        id: "T-852",
        kind: ChangeKind::Mark(Mark::Stale),
        text: "gate went stale on af7a6dc",
    },
    Change {
        at: "21:59",
        id: "T-868",
        kind: ChangeKind::Message,
        text: "worker replied in thread on T-868/R1/Q1",
    },
    Change {
        at: "21:17",
        id: "T-871",
        kind: ChangeKind::Start,
        text: "worker started · pi-sol xhigh",
    },
    Change {
        at: "21:11",
        id: "T-890",
        kind: ChangeKind::Note,
        text: "checkpoint 3 captured",
    },
    Change {
        at: "20:40",
        id: "T-867",
        kind: ChangeKind::Start,
        text: "worker started · pi-sol xhigh",
    },
    Change {
        at: "20:32",
        id: "T-890",
        kind: ChangeKind::Note,
        text: "checkpoint 2 captured",
    },
    Change {
        at: "19:33",
        id: "T-894",
        kind: ChangeKind::Start,
        text: "worker started · pi-sol xhigh",
    },
    Change {
        at: "19:31",
        id: "T-890",
        kind: ChangeKind::Start,
        text: "worker started · pi-sol xhigh",
    },
    Change {
        at: "19:12",
        id: "T-893",
        kind: ChangeKind::Mark(Mark::Done),
        text: "you accepted plan T-885 P2 → 3 members published",
    },
];

pub(super) static HUNK: Hunk = Hunk {
    file: "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch",
    at: "@@ -1959,13 +1990,16 @@ class AgentSession",
    start: (1990, 1990),
    lines: &[
        (
            LineKind::Context,
            "     * @returns Object with steering and followUp arrays",
        ),
        (LineKind::Context, "     */"),
        (LineKind::Del, "    clearQueue() {"),
        (
            LineKind::Del,
            "        const steering = [...this._steeringMessages];",
        ),
        (
            LineKind::Del,
            "        const followUp = [...this._followUpMessages];",
        ),
        (LineKind::Del, "        this.agent.clearAllQueues();"),
        (LineKind::Add, "    clearQueue(options) {"),
        (LineKind::Add, "        const steering = [], followUp = [];"),
        (
            LineKind::Add,
            "        for (const message of this.agent.getQueuedMessages()) {",
        ),
        (
            LineKind::Add,
            "            const tag = this._queueTags.get(message);",
        ),
        (
            LineKind::Add,
            "            const restored = options?.restore && this._queueObserver?.restoreInput",
        ),
        (
            LineKind::Add,
            "                ? this._queueObserver.restoreInput(tag) : undefined;",
        ),
        (
            LineKind::Add,
            "            if (options?.restore && restored === undefined) continue;",
        ),
        (
            LineKind::Add,
            "            this.agent.removeQueuedMessage(message, options?.restore ? \"pane_restore\" : \"host\");",
        ),
        (LineKind::Add, "        }"),
        (LineKind::Context, "        return { steering, followUp };"),
    ],
};

#[cfg(test)]
mod tests {
    use super::TASKS;
    use std::collections::HashSet;

    #[test]
    fn duplicate_task_ids_shadow_find_results() {
        let mut ids = HashSet::new();
        for task in TASKS {
            assert!(ids.insert(task.id), "duplicate task id: {}", task.id);
        }
    }
}

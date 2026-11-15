use super::*;

pub(super) fn question(feed: &mut Feed) {
    let key = "T-868/R1/Q1";
    let at = stamp(feed, feed.now.saturating_sub(10800));
    let text = "Restore policy: pane-origin input only, or all origins?";
    feed.inbox.insert(
        key.into(),
        InboxRow {
            id: key.into(),
            kind: InboxKind::WorkerQuestion,
            task: Some("T-868".into()),
            plan: Some("T-870".into()),
            project: Some("corral".into()),
            title: text.into(),
            from: "worker T-868 R1 · preview".into(),
            asked_at: Some(at.clone()),
            needed_by: None,
            blocking: true,
            state: InboxState::Open,
            question_rev: Some(2),
            answerable: true,
            unanswerable_reason: None,
            attention: Some(AttentionMove {
                whose_move: AttentionActor::Owner,
                reason: "Restore policy awaits your decision".into(),
                next_step: "Answer T-868/R1/Q1".into(),
                command: Some("corral question answer T-868/R1/Q1".into()),
                destination: AttentionDestination::Inbox,
            }),
            rev: 1,
        },
    );
    let options = [
        ("Restore only pane-origin input; leave Corral/extension items queued", "Worker adds the restoreInput hook: pane items return their original text, others stay queued. Then rebase and quick gate.", "≈1 step · $2–4 estimated", true),
        ("Retain restore-all; journal removals and treat resubmission as new pane input", "Worker journals every removal and re-admits restored text as new pane input with fresh identity.", "≈2 steps · $6–9 estimated", false),
    ].into_iter().map(|(label, next, cost, recommended)| QuestionOption {
        label: label.into(), next: Some(next.into()), unblocks: vec!["T-868 step 3/5".into()],
        cost: Some(cost.into()), recommended, evidence: Some(0), consequence_missing: false,
        consequence_derived: false,
    }).collect();
    put(feed, DetailRef::Question { key: key.into() }, DetailBody::Question(QuestionDetail {
        key: key.into(), rev: 2, state: InboxState::Open, kind: InboxKind::WorkerQuestion,
        blocking: true, task: Some("T-868".into()), plan: Some("T-870".into()),
        from: QuestionFrom { role: "worker".into(), session: Some("T-868".into()), profile: Some("pi-sol xhigh".into()) },
        asked_at: at.clone(), needed_by: None,
        text: "T-701@P2 leaves one restore-policy decision open for T-868: on stock/plain paths where Esc/dequeue restoration is enabled, should restoration remove only pane-origin queued input, or restore all origins? Rich mode will remain read-only/count-only with no restoration either way.".into(),
        options, recommendation: Some("Restore only pane-origin input, so Corral/extension instructions cannot become an owner draft. The ledger work can continue independently.".into()),
        place: QuestionPlace {
            goal: Some("Stable queue identities and durable lifecycle".into()),
            position: Some(PlanPosition { plan: "T-870".into(), layer: 1, layers: 2, landed: 0, total: 3 }),
            waiting: vec![QuestionWaiting { id: "T-868".into(), title: "Blocked until you answer".into(), kind: WaitKind::You },
                QuestionWaiting { id: "T-869".into(), title: "Independent · after T-867, not this answer".into(), kind: WaitKind::Info }],
        },
        evidence: vec![EvidenceBlock { source_ref: "preview:restore-policy".into(), title: "Preview · HostQueueObserver".into(),
            body: EvidenceBody::Excerpt { path: Some("packages/corral-pi/src/host/queue.ts".into()), start_line: Some(18),
                lines: vec!["interface HostQueueObserver {".into(), "  restoreInput?(tag: unknown): string | undefined;".into(), "}".into()], truncated: false } }],
        thread: vec![Followup { by: "owner".into(), via: "preview:phone".into(), at: at.clone(), text: "Does option 1 change anything in rich mode?".into() },
            Followup { by: "worker T-868".into(), via: "preview".into(), at: at.clone(), text: "No. Rich mode stays read-only/count-only; neither option restores there.".into() }],
        history: vec![QuestionHistory { by: "owner".into(), via: Some("preview".into()), at,
            text: "Approved T-701 P2: keep rich readOnlyQueue; leave restore-policy choice open where restoration is enabled.".into(), source_ref: Some("T-701@P2".into()) }], answer: None, deadline: None,
    }));
    if let Some(mut row) = feed.board.task("T-868").cloned() {
        row.questions = 1;
        row.attention.push(key.into());
        save_task(feed, row);
    }
}

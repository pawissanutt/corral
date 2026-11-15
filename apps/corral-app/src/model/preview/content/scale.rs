use super::*;

pub(super) fn scale(feed: &mut Feed) {
    let stems = [
        "schema", "codec", "index", "cursor", "pager", "ledger", "replay", "fence", "lease",
        "probe", "bridge", "relay", "sweep", "audit", "quota", "notice",
    ];
    let mut members = Vec::with_capacity(64);
    let mut n = 0;
    let mut previous: Vec<String> = Vec::new();
    let mut edges = Vec::new();
    for (layer, size) in [14, 12, 10, 11, 9, 8].into_iter().enumerate() {
        let mut current = Vec::with_capacity(size);
        for position in 0..size {
            n += 1;
            let id = format!("S-{}", 900 + n);
            let title = format!("{}-{}", stems[n * 7 % stems.len()], n * 13 % 97);
            let group = if layer == 0 || (layer == 1 && position != 5 && position != 6) {
                Group::Other
            } else if (layer == 1 && position == 5) || (layer == 2 && position < 2) {
                Group::Verifying
            } else if (layer == 1 && position == 6) || (layer == 2 && position == 2) {
                Group::NeedsYou
            } else if layer == 3 && position == 0 {
                Group::Held
            } else {
                Group::Queued
            };
            let row = task(
                feed,
                &id,
                &title,
                Some("S-900"),
                group,
                match group {
                    Group::Other => "landed",
                    Group::Held => "held · budget",
                    Group::NeedsYou => "needs owner decision",
                    Group::Verifying => "gating",
                    _ => "waiting on previous layer",
                },
            );
            members.push(PlanMember {
                id: id.clone(),
                title,
                layer: Some(layer as u32),
                state: row.now_code.clone(),
                rail: stages(&row),
            });
            if let Some(from) = previous.first() {
                edges.push(PlanEdge {
                    from: from.clone(),
                    to: id.clone(),
                });
            }
            current.push(id);
            save_task(feed, row);
        }
        previous = current;
    }
    feed.plans.insert(
        "S-900".into(),
        PlanRow {
            id: "S-900".into(),
            title: "Preview · 64-member tracker".into(),
            role: Some(PlanRole::Tracker),
            linked: None,
            state: PlanState::Executing,
            revision: None,
            delivery: Some("main".into()),
            members_total: 64,
            members_landed: 24,
            blockers: vec![],
            accepted_at: None,
            rev: 1,
        },
    );
    put(
        feed,
        DetailRef::Plan { id: "S-900".into() },
        DetailBody::Plan(PlanDetail {
            id: "S-900".into(),
            role: Some(PlanRole::Tracker),
            linked: None,
            revision: None,
            revisions: vec![],
            owner_decisions: vec![],
            state: PlanState::Executing,
            document: Some(PlanDocument {
                problem: Some("Preview · dense dependency graphs must stay navigable".into()),
                goal: Some("64 synthetic members across six layers".into()),
                changes: vec![],
                retained: vec![],
                non_goals: vec![],
                proposed: vec![],
            }),
            reviews: vec![],
            members,
            edges,
            scenarios: Some(64),
            mapped: Some(64),
            acceptable: false,
            accept_blockers: vec!["Preview · members still in progress".into()],
        }),
    );
    refresh_plans(feed);
}

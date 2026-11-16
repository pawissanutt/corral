use super::*;

pub(super) fn plan(feed: &mut Feed, command: &Plan) -> Result<(), String> {
    if matches!(command, Plan::AddMember { .. } | Plan::MoveMember { .. }) {
        return super::control::membership(feed, command);
    }
    let id = match command {
        Plan::Accept { id, .. } | Plan::RequestChanges { id, .. } | Plan::Cancel { id, .. } => id,
        _ => return unsupported(command.verb()),
    };
    let target = DetailRef::Plan { id: id.clone() };
    let Some(DetailBody::Plan(mut detail)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    else {
        return Err("Preview plan not found".into());
    };
    match command {
        Plan::Accept { revision, .. } => {
            if detail.revision.as_ref() != Some(revision) || !detail.acceptable {
                return Err("conflict: preview plan revision is not acceptable".into());
            }
            detail.state = PlanState::Approved;
            detail.acceptable = false;
            if id == "T-885" {
                detail.linked = Some("T-893".into());
            }
        }
        Plan::RequestChanges { revision, text, .. } => {
            if detail.revision.as_ref() != Some(revision) {
                return Err("conflict: preview plan revision changed".into());
            }
            detail.state = PlanState::Drafting;
            detail.acceptable = false;
            detail.accept_blockers.push(text.clone());
        }
        Plan::Cancel { .. } => {
            detail.state = PlanState::Cancelled;
            detail.acceptable = false;
        }
        _ => return unsupported(command.verb()),
    }
    if let Some(row) = feed.plans.get_mut(id.as_str()) {
        row.state = detail.state;
        row.linked = detail.linked.clone();
        row.rev += 1;
    }
    put(feed, target, DetailBody::Plan(detail));
    Ok(())
}
pub(super) fn review(feed: &mut Feed, command: &Review) -> Result<(), String> {
    let id = match command {
        Review::Approve { id, .. }
        | Review::RequestChanges { id, .. }
        | Review::Verdict { id, .. }
        | Review::Discard { id } => id,
        _ => return unsupported(command.verb()),
    };
    let target = DetailRef::Review { task: id.clone() };
    let candidate = feed.detail(&target).and_then(|cache| cache.body()).cloned();
    let Some(DetailBody::Review(mut detail)) = candidate else {
        if matches!(command, Review::Discard { .. }) {
            return super::control::discard(feed, id);
        }
        return Err("Preview candidate not found".into());
    };
    let verdict = match command {
        Review::Approve { sha, .. } | Review::Verdict { sha, .. } => {
            if detail.candidate_sha.as_ref() != Some(sha) {
                return Err("conflict: preview candidate changed".into());
            }
            if matches!(command, Review::Approve { .. }) && !detail.approvable {
                return Err("Preview candidate is not approvable".into());
            }
            if let Review::Verdict { verdict, .. } = command {
                verdict.clone()
            } else {
                "approve".into()
            }
        }
        Review::RequestChanges { .. } => "changes".into(),
        Review::Discard { .. } => "discarded".into(),
        _ => return unsupported(command.verb()),
    };
    detail.verdicts.push(ReviewVerdict {
        by: "owner · preview".into(),
        sha: detail.candidate_sha.clone().unwrap_or_default(),
        verdict: verdict.clone(),
        at: Some(stamp(feed, feed.now)),
        text: if let Review::RequestChanges { text, .. } = command {
            Some(text.clone())
        } else {
            None
        },
        stale: false,
    });
    detail.state = verdict.clone();
    detail.approvable = false;
    put(feed, target, DetailBody::Review(detail));
    let at = stamp(feed, feed.now);
    for item in feed.inbox.values_mut().filter(|item| {
        item.kind == InboxKind::Review
            && item.task.as_ref() == Some(id)
            && item.state == InboxState::Open
    }) {
        item.state = InboxState::Answered {
            by: "owner".into(),
            via: "preview:local-only".into(),
            at: at.clone(),
        };
        item.rev += 1;
    }
    if let Some(mut row) = feed.board.task(id).cloned() {
        row.now = format!("review {verdict} · preview");
        row.rev += 1;
        row.attention
            .retain(|key| !key.starts_with(&format!("review:{id}:")));
        if verdict == "approve" {
            row.rail[2].mark = crate::model::board::Mark::Done;
            row.group = Group::Verifying;
        }
        if verdict == "changes" {
            row.group = Group::Producing;
        }
        content::save_task(feed, row);
    }
    if matches!(command, Review::Discard { .. }) {
        return super::control::discard(feed, id);
    }
    Ok(())
}
pub(super) fn annotation(feed: &mut Feed, command: &Annotation) -> Result<(), String> {
    let id = match command {
        Annotation::Open { id, .. }
        | Annotation::Reply { id, .. }
        | Annotation::Resolve { id, .. }
        | Annotation::Reopen { id, .. } => id,
        _ => return unsupported(command.verb()),
    };
    let target = DetailRef::Review { task: id.clone() };
    let Some(DetailBody::Review(mut detail)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    else {
        return Err("Preview candidate not found".into());
    };
    let at = stamp(feed, feed.now);
    match command {
        Annotation::Open {
            key,
            expected_run,
            base,
            head,
            path,
            side,
            range,
            text,
            ..
        } => {
            if detail.candidate_sha.as_ref() != Some(head)
                || detail.base.as_ref() != Some(base)
                || expected_run != "R1"
            {
                return Err("conflict: preview candidate changed".into());
            }
            detail.annotations.push(ReviewAnnotation {
                id: key.clone(),
                revision: 1,
                run: expected_run.clone(),
                base: base.clone(),
                head: head.clone(),
                path: path.clone(),
                side: *side,
                range: range.clone(),
                resolved: false,
                stale: false,
                messages: vec![AnnotationMessage {
                    by: "owner · preview".into(),
                    at,
                    text: text.clone(),
                }],
            });
        }
        Annotation::Reply {
            thread,
            expected_revision,
            text,
            ..
        }
        | Annotation::Resolve {
            thread,
            expected_revision,
            text,
            ..
        }
        | Annotation::Reopen {
            thread,
            expected_revision,
            text,
            ..
        } => {
            let thread = detail
                .annotations
                .iter_mut()
                .find(|annotation| &annotation.id == thread)
                .ok_or("Preview annotation not found")?;
            if thread.revision != *expected_revision {
                return Err("conflict: preview annotation changed".into());
            }
            if matches!(command, Annotation::Resolve { .. }) {
                thread.resolved = true;
            }
            if matches!(command, Annotation::Reopen { .. }) {
                thread.resolved = false;
            }
            thread.revision += 1;
            thread.messages.push(AnnotationMessage {
                by: "owner · preview".into(),
                at,
                text: text.clone(),
            });
        }
        _ => return unsupported(command.verb()),
    }
    put(feed, target, DetailBody::Review(detail));
    Ok(())
}

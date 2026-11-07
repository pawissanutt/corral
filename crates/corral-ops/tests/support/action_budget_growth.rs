//! History growth for the action budget's stores (plan T-253 section 4): every history family
//! of the acted task, its relations and terminal tasks, and the emitted rows its prompts show.
use std::path::Path as FsPath;

use corral_core::{CommentAuthor, Run, Status, Store, TaskComment};
use serde_json::json;

use super::action_budget_fixture::Kind;
use super::fixture;

pub(super) fn bare_historic(index: usize, worktree: &FsPath) -> Run {
    serde_json::from_value(json!({
        "id": format!("H{index:03}"), "agent": "historic", "harness": "historic",
        "worktree": worktree.join(format!("history-{index:03}")), "branch": "corral/history",
        "base": "main", "base_sha": "0".repeat(40), "started": fixture::now(),
        "ended": fixture::now(), "outcome": "superseded",
        "review": {"reviewer": "historic", "sha": "0".repeat(40), "verdict": "approve",
                   "at": fixture::now()},
    }))
    .unwrap()
}

/// A finished receipt of run `run` for obligation `id`.
pub(super) fn receipt(
    run: &str,
    id: &str,
    outcome: corral_core::ReceiptOutcome,
) -> corral_core::ResumeReceipt {
    corral_core::ResumeReceipt {
        version: 1,
        id: format!("{id}-receipt"),
        run: run.into(),
        outcome,
        delivery: corral_core::ResumeDelivery::Acknowledged,
        observed_at: fixture::now(),
        answer: (outcome == corral_core::ReceiptOutcome::Answered).then(|| "main".into()),
        data: json!({}),
    }
}

/// A checkpoint record of run `run` at `revision`, freezing `frozen`.
pub(super) fn checkpoint_record(
    run: &str,
    revision: u64,
    frozen: &corral_core::RunObligations,
) -> corral_core::CheckpointRecord {
    serde_json::from_value(json!({
        "artifact": format!("artifacts/history/{run}-checkpoint-{revision}.json"),
        "checkpoint": {
            "version": 1, "revision": revision, "progress": null, "completed_steps": [],
            "current_step": null, "captured": fixture::now(), "run_obligations": frozen,
            "input": {
                "task": "T-000", "run": run, "spec_revision": "0".repeat(64),
                "expected_revision": 0, "progress_revision": 0, "head": "0".repeat(40),
                "base": "0".repeat(40), "tree": "0".repeat(64), "criterion_ids": [],
                "evidence_refs": [], "next_action": "Earlier work", "decisions": [],
                "waits": [], "resources": [],
            },
        },
    }))
    .unwrap()
}

/// A resolved wait and an answered decision of run `run`: final obligations.
pub(super) fn final_obligations(
    run: &str,
    i: usize,
) -> (corral_core::WaitRecord, corral_core::DecisionRecord) {
    use corral_core::{ObligationSource, ReceiptOutcome, WaitObservation, WaitState};
    let observation = WaitObservation {
        at: fixture::now(),
        state: WaitState::Ready,
        detail: "resolved earlier".into(),
    };
    let wait = format!("{run}-build-{i:03}");
    let decision = format!("{run}-branch-{i:03}");
    (
        corral_core::WaitRecord {
            request: corral_core::WaitRequest {
                version: 1,
                id: wait.clone(),
                run: run.into(),
                condition: "An earlier build completes".into(),
                owner: "fixture".into(),
                source: ObligationSource::Worker,
                deadline: None,
                handle: corral_core::WaitHandle::Channel {
                    channel_id: wait.clone(),
                    capability: None,
                },
            },
            registered_at: fixture::now(),
            observation: observation.clone(),
            receipt: Some(receipt(run, &wait, ReceiptOutcome::Ready)),
            wake_message: None,
        },
        corral_core::DecisionRecord {
            request: corral_core::DecisionRequest {
                version: 1,
                id: decision.clone(),
                run: run.into(),
                question: "Which earlier branch?".into(),
                requested_action: "Choose a branch".into(),
                owner: "owner".into(),
                source: ObligationSource::Worker,
                deadline: None,
            },
            registered_at: fixture::now(),
            observation,
            receipt: Some(receipt(run, &decision, ReceiptOutcome::Answered)),
        },
    )
}

/// An earlier coding run with nested history of its own: a review child, a switch, an older
/// checkpoint, a resolved wait, an answered decision, applied inbox effects and an advanced
/// observe cursor. Nothing an action takes reads any of it.
pub(super) fn historic_run(index: usize, worktree: &FsPath) -> Run {
    let mut run = bare_historic(index, worktree);
    run.gate = Some(
        serde_json::from_value(json!({
            "result": "pass", "at": fixture::now(), "sha": "0".repeat(40),
            "commands": [], "log_sha256": "0".repeat(64),
        }))
        .unwrap(),
    );
    let mut child = bare_historic(index, worktree);
    child.id = format!("{}-review-1", run.id);
    child.outcome = Some(corral_core::RunOutcome::Done);
    run.review_runs.push(child);
    run.switches.push(corral_core::ProfileSwitch {
        from: "historic-a".into(),
        to: "historic-b".into(),
        reason: "earlier switch".into(),
    });
    run.checkpoints
        .push(checkpoint_record(&run.id, 1, &Default::default()));
    let (wait, decision) = final_obligations(&run.id, 0);
    run.obligations.waits.push(wait);
    run.obligations.decisions.push(decision);
    run.inbox.effects_applied.insert(1);
    run.inbox.next_seq = 2;
    run.inbox.observe_cursor.offset = 64;
    run.inbox.observe_cursor.last_complete_line = 1;
    run
}

/// `scale` earlier rows in every history family of `id` that no current input reads, and
/// nested growth before each current container's current child.
/// A Done relation whose only coding runs are grown history landed its last one: that run is Done and its
/// acceptance receipt names the project's `main`, which every dispatch base contains.
pub(super) fn land_grown(store: &Store, id: &str) {
    let task = store.get(id).unwrap();
    // A relation with runs of its own keeps its real acceptance receipt.
    let grown_only = task.runs.iter().all(|run| run.id.starts_with('H'));
    let Some(run) = task
        .runs
        .last()
        .filter(|_| task.status == Status::Done && grown_only)
    else {
        return;
    };
    let run = run.id.clone();
    store
        .update(id, |t| {
            t.runs.last_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let repo = store.project_path("app").unwrap();
    let sha = corral_git::rev_sha(&repo, "main").unwrap();
    store
        .log_event(
            Some(id),
            "approved",
            json!({"run": run, "sha": sha, "strategy": "fast_forward", "base": "main"}),
        )
        .unwrap();
}

/// A report of earlier run `run`: a real snapshot under the store's artifacts, its digest and
/// size recorded as an attach would.
fn historic_artifact(store: &Store, task: &str, run: &str) -> corral_core::Artifact {
    let bytes = format!("Earlier report of {task} {run}\n");
    let sha = crate::verification::digest(bytes.as_bytes());
    let path = format!("artifacts/{task}/{run}/{}-report.md", &sha[..8]);
    let file = store.dir().join(&path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    serde_json::from_value(json!({
        "run": run, "kind": "report", "title": format!("Earlier report {run}"), "path": path,
        "source": file, "sha256": sha, "bytes": bytes.len(), "added": fixture::now(),
    }))
    .unwrap()
}

/// The growth [`grow`] claims is really there: every history family of `id` reaches `scale`.
pub(super) fn grown_by(store: &Store, id: &str, scale: usize, acted: bool, children: bool) {
    let task = store.get(id).unwrap();
    let historic: Vec<&Run> = task.runs.iter().filter(|r| r.id.starts_with('H')).collect();
    let nested = |f: fn(&Run) -> usize| historic.iter().map(|r| f(r)).sum::<usize>();
    assert!(
        task.approved_scope_history.len() >= scale,
        "{id}: scope revisions"
    );
    if !acted || task.runs.len() > historic.len() {
        assert!(historic.len() >= scale, "{id}: earlier coding runs");
        for (family, count) in [
            ("review children", nested(|r| r.review_runs.len())),
            ("switches", nested(|r| r.switches.len())),
            ("older checkpoints", nested(|r| r.checkpoints.len())),
            ("final waits", nested(|r| r.obligations.waits.len())),
            (
                "answered decisions",
                nested(|r| r.obligations.decisions.len()),
            ),
            ("applied effects", nested(|r| r.inbox.effects_applied.len())),
            (
                "passed gate records",
                historic
                    .iter()
                    .filter(|r| {
                        r.gate
                            .as_ref()
                            .is_some_and(|g| g.result == corral_core::GateResult::Pass)
                    })
                    .count(),
            ),
            (
                "earlier artifacts",
                task.artifacts
                    .iter()
                    .filter(|a| a.run.starts_with('H') && store.dir().join(&a.path).is_file())
                    .count(),
            ),
        ] {
            assert!(count >= scale, "{id}: {family} {count} < {scale}");
        }
    }
    // The current container itself grew, wherever it is a real run.
    if let Some(run) = task.runs.last().filter(|run| !run.id.starts_with('H')) {
        let usage = |r: &Run| r.usage.as_ref().map_or(0, |u| u.entries.len());
        let grown: Vec<&Run> = run
            .review_runs
            .iter()
            .filter(|c| c.id.contains("-review-h"))
            .collect();
        let mut families = vec![
            ("current usage entries", usage(run)),
            ("current switches", run.switches.len().saturating_sub(1)),
            ("current applied effects", run.inbox.effects_applied.len()),
            ("earlier-binding cursors", run.inbox.observe_cursors.len()),
        ];
        if children {
            families.push(("current review children", grown.len()));
            families.push((
                "current children's usage",
                grown.iter().map(|c| usage(c)).sum(),
            ));
            // A child's pane is on its run's session: a run without a pane has none.
            families.push((
                "current children's panes",
                if run.herdr.is_some() {
                    grown.iter().filter(|c| c.herdr.is_some()).count()
                } else {
                    scale
                },
            ));
        }
        if run.checkpoints.is_empty() {
            families.push(("current final waits", run.obligations.waits.len()));
            families.push((
                "current answered decisions",
                run.obligations.decisions.len(),
            ));
        } else {
            families.push(("older checkpoints", run.checkpoints.len() - 1));
        }
        for (family, count) in families {
            assert!(
                count >= scale,
                "{id} {}: {family} {count} < {scale}",
                run.id
            );
        }
    }
}

/// Zero-token, unpriced usage: the totals stay as they were.
fn zero_usage(scale: usize) -> Vec<corral_core::UsageEntry> {
    (0..scale)
        .map(|i| corral_core::UsageEntry {
            connection: None,
            billing: None,
            at: Some(fixture::now()),
            latency_ms: Some(1),
            model: Some(format!("historic-{i:03}")),
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reported_usd: None,
            estimated_usd: None,
            price_source: None,
            unpriced_tokens: 0,
        })
        .collect()
}

/// Nested history of a current coding container that keeps its legal state and read facts:
/// older incompatible checkpoints behind the newest (the last record stays newest), usage,
/// finished review children with their own usage and closed pane before any current child,
/// switches before the last, applied inbox effects below the next sequence, and cursors of
/// earlier session bindings. Final waits and decisions go only on a run without a checkpoint:
/// a checkpointed run's obligations must equal its newest checkpoint's frozen ones
/// (`resume::checkpoint_current`), and growing both is the emitted variant.
fn grow_current(run: &mut Run, scale: usize, root: &FsPath, children: bool) {
    if let Some(newest) = run.checkpoints.pop() {
        run.checkpoints
            .extend((0..scale).map(|_| checkpoint_record(&run.id, 0, &Default::default())));
        run.checkpoints.push(newest);
    } else {
        for i in 0..scale {
            let (wait, decision) = final_obligations(&run.id, i);
            run.obligations.waits.push(wait);
            run.obligations.decisions.push(decision);
        }
    }
    run.usage
        .get_or_insert_with(Default::default)
        .entries
        .extend(zero_usage(scale));
    // A reviewer's pane is on the store's bound herdr session, as the run's own pane is.
    let socket = serde_json::to_value(&run.herdr).unwrap()["socket"].clone();
    let grown = (0..scale).map(|i| {
        let mut child = bare_historic(i, root);
        child.id = format!("{}-review-h{i:03}", run.id);
        child.outcome = Some(corral_core::RunOutcome::Done);
        child
            .usage
            .get_or_insert_with(Default::default)
            .entries
            .extend(zero_usage(1));
        if !socket.is_null() {
            child.herdr = Some(
                serde_json::from_value(json!({
                    "workspace_id": format!("historic-w{i:03}"),
                    "pane_id": format!("historic-w{i:03}:p1"),
                    "socket": socket,
                }))
                .unwrap(),
            );
        }
        child
    });
    if children {
        run.review_runs.splice(0..0, grown.collect::<Vec<_>>());
    }
    let last = run.switches.pop();
    run.switches
        .extend((0..scale).map(|i| corral_core::ProfileSwitch {
            from: format!("historic-{i}"),
            to: format!("historic-{}", i + 1),
            reason: "earlier switch".into(),
        }));
    run.switches.extend(last.or_else(|| {
        Some(corral_core::ProfileSwitch {
            from: "historic-last".into(),
            to: run.profile.clone().unwrap_or_default(),
            reason: "earlier switch".into(),
        })
    }));
    let next = run.inbox.next_seq.max(1);
    run.inbox.effects_applied.extend(next..next + scale as u64);
    run.inbox.next_seq = next + scale as u64;
    run.inbox
        .observe_cursors
        .extend((0..scale).map(|i| corral_core::inbox::SessionCursor {
            adapter: "historic".into(),
            binding: corral_core::inbox::SessionBinding {
                session: format!("historic-session-{i:03}"),
                endpoint: None,
                transcript: None,
            },
            cursor: corral_core::inbox::ObserveCursor {
                offset: 64,
                last_complete_line: 1,
                coverage: Default::default(),
            },
        }));
}

/// `scale` of the acted task's current review child's own nested rows: usage, switches,
/// applied inbox effects and observe cursors. A hot action that finishes or edits that child
/// must not read them (review R2-1).
pub(super) fn grow_current_child(store: &Store, id: &str, scale: usize) {
    store
        .update(id, |t| {
            let Some(child) = t
                .current_run_mut()
                .and_then(|run| run.review_runs.last_mut())
            else {
                return Ok(());
            };
            // One model: the child's projected totals keep one size at every scale.
            child
                .usage
                .get_or_insert_with(Default::default)
                .entries
                .extend(
                    zero_usage(scale)
                        .into_iter()
                        .map(|entry| corral_core::UsageEntry {
                            model: Some("historic".into()),
                            ..entry
                        }),
                );
            let last = child.switches.pop();
            child
                .switches
                .extend((0..scale).map(|i| corral_core::ProfileSwitch {
                    // One width: the newest switch stays in the child's row at every scale.
                    from: format!("historic-{i:03}"),
                    to: format!("historic-{:03}", i + 1),
                    reason: "earlier switch".into(),
                }));
            child.switches.extend(last);
            let next = child.inbox.next_seq.max(1);
            child
                .inbox
                .effects_applied
                .extend(next..next + scale as u64);
            child.inbox.next_seq = next + scale as u64;
            child.inbox.observe_cursors.extend((0..scale).map(|i| {
                corral_core::inbox::SessionCursor {
                    adapter: "historic".into(),
                    binding: corral_core::inbox::SessionBinding {
                        session: format!("historic-child-session-{i:03}"),
                        endpoint: None,
                        transcript: None,
                    },
                    cursor: corral_core::inbox::ObserveCursor {
                        offset: 64,
                        last_complete_line: 1,
                        coverage: Default::default(),
                    },
                }
            }));
            Ok(())
        })
        .unwrap();
}

/// `scale` of every history family on task `id`. The acted task gets earlier coding runs only
/// beside its own (its actions decide on whether it has run); every other task gets them always.
pub(super) fn grow(
    store: &Store,
    root: &FsPath,
    id: &str,
    scale: usize,
    acted: bool,
    children: bool,
) {
    store
        .update(id, |t| {
            if !acted || !t.runs.is_empty() {
                let earlier: Vec<Run> = (0..scale).map(|i| historic_run(i, root)).collect();
                let artifacts: Vec<_> = earlier
                    .iter()
                    .map(|run| historic_artifact(store, &t.id, &run.id))
                    .collect();
                t.runs.splice(0..0, earlier);
                t.artifacts.splice(0..0, artifacts);
            }
            if let Some(current) = t.plan_runs.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut run = current.clone();
                    run.id = format!("HD{i:03}");
                    run.state = corral_core::PlanRunState::Cancelled;
                    run.ended = Some(fixture::now());
                    run.herdr = None;
                    run.inbox = Default::default();
                    run
                });
                let at = t.plan_runs.len() - 1;
                t.plan_runs.splice(at..at, earlier.collect::<Vec<_>>());
            }
            if let Some(current) = t.plans.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut plan = current.clone();
                    plan.id = format!("HP{i:03}");
                    plan.reviews.clear();
                    plan.approval = None;
                    plan
                });
                let at = t.plans.len() - 1;
                t.plans.splice(at..at, earlier.collect::<Vec<_>>());
                let plan = t.plans.last_mut().unwrap();
                if let Some(template) = plan.reviews.last().cloned() {
                    let reviews = (0..scale).map(|i| {
                        let mut review = template.clone();
                        review.id = format!("HPR{i:03}");
                        review.reviewer = "historic".into();
                        review.verdict = Some(corral_core::Verdict::Approve);
                        review.notes = None;
                        review.herdr = None;
                        review.pending_reply = None;
                        review.inbox = Default::default();
                        review
                    });
                    plan.reviews.splice(0..0, reviews.collect::<Vec<_>>());
                }
            }
            if let Some(current) = t.aggregate_attempts.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut attempt = current.clone();
                    attempt.id = format!("HA{i:03}");
                    attempt.acceptance = None;
                    attempt
                });
                let at = t.aggregate_attempts.len() - 1;
                t.aggregate_attempts
                    .splice(at..at, earlier.collect::<Vec<_>>());
                let attempt = t.aggregate_attempts.last_mut().unwrap();
                if let Some(template) = attempt.reviews.last().cloned() {
                    let reviews = (0..scale).map(|i| {
                        let mut review = template.clone();
                        review.id = format!("{}-H{i:03}", attempt.id);
                        review.notes = None;
                        review.herdr = None;
                        review
                    });
                    attempt.reviews.splice(0..0, reviews.collect::<Vec<_>>());
                }
            }
            if let Some(run) = t.runs.last_mut().filter(|run| !run.id.starts_with('H')) {
                grow_current(run, scale, root, children);
            }
            Ok(())
        })
        .unwrap();
    // A cleanup released the finished children's panes long ago.
    let keys: Vec<String> = store
        .head(id)
        .unwrap()
        .unreleased_panes(false)
        .unwrap()
        .into_iter()
        .filter(|row| row.pane.starts_with("historic-"))
        .map(|row| row.key)
        .collect();
    if !keys.is_empty() {
        store
            .update_head(id, |h| h.release_panes(&keys, &[]))
            .unwrap();
    }
    // Scope amendments: a plain update may not write them.
    let mut task = store.get(id).unwrap();
    let first = u64::try_from(task.approved_scope_history.len()).unwrap();
    task.approved_scope_history
        .extend(
            (0..scale as u64).map(|i| corral_core::ApprovedScopeRevision {
                revision: first + i + 1,
                approved_scope: None,
                superseded_at: fixture::now(),
            }),
        );
    store.fixture_put_task(&task).unwrap();
    for i in 0..scale {
        let run = format!("H{i:03}");
        for kind in ["fixture_history", "lessons_applied", "gate_started"] {
            store
                .log_event(
                    Some(id),
                    kind,
                    json!({"run": run, "lessons": [], "pane": "h"}),
                )
                .unwrap();
        }
    }
}

/// Emitted history: owner comments and non-approve findings with notes on earlier plans and
/// review children.
pub(super) fn emit(store: &Store, id: &str, scale: usize, kind: Kind) {
    store
        .update(id, |t| {
            for i in 0..scale + 1 {
                t.comments.push(TaskComment {
                    at: fixture::now(),
                    author: CommentAuthor::Owner,
                    text: format!("Owner note {i:02}"),
                });
            }
            // A non-approve finding on each earlier plan: a prompt's Previous plan findings.
            let count = t.plans.len();
            for plan in t.plans.iter_mut().take(count.saturating_sub(1)) {
                plan.reviews.push(serde_json::from_value(json!({
                    "id": format!("{}-finding", plan.id), "reviewer": "historic",
                    "policy_digest": "0".repeat(64), "verdict": "changes",
                    "notes": format!("Finding on {}", plan.id), "at": fixture::now(),
                }))?);
            }
            // Non-approve findings on earlier runs' review children: a review packet's
            // previous findings, which none of these kinds builds.
            // A hot kind's failure streak reads the run before the current one by key: its
            // facts stay as they are.
            let previous = t.runs.len().checked_sub(2);
            for (index, run) in t.runs.iter_mut().enumerate() {
                if !run.id.starts_with('H') || (kind.hot() && Some(index) == previous) {
                    continue;
                }
                for child in &mut run.review_runs {
                    if let Some(review) = child.review.as_mut() {
                        review.verdict = Some(corral_core::Verdict::Changes);
                        review.notes = Some(format!("Finding on {}", child.id));
                    }
                    // A superseded finding is emitted but leaves the unsuperseded-changes facts
                    // a hot kind's checks read as they are.
                    if kind.hot() {
                        child.outcome = Some(corral_core::RunOutcome::Superseded);
                    }
                }
            }
            // The repaired run's final obligations, frozen into its newest checkpoint.
            if matches!(kind, Kind::RepairLegacy | Kind::RepairInbox) {
                let id = t.id.clone();
                let run = t
                    .current_run_mut()
                    .ok_or_else(|| anyhow::anyhow!("repair without a run"))?;
                for i in 0..10 {
                    let (wait, decision) = final_obligations(&run.id, i);
                    run.obligations.waits.push(wait);
                    if i == 0 {
                        run.obligations.decisions.push(decision);
                    }
                }
                let frozen = run.obligations.clone();
                let revision = run.checkpoints.last().map_or(0, |c| c.checkpoint.revision) + 1;
                let mut record = checkpoint_record(&run.id, revision, &frozen);
                // Named like a saved checkpoint, so the head's newest meta keeps its size.
                record.artifact = format!(
                    "artifacts/{}/{}/{}-checkpoint-{revision}.json",
                    id,
                    run.id,
                    "0".repeat(64)
                );
                run.checkpoints.push(record);
            }
            Ok(())
        })
        .unwrap();
}

//! Durable claim-before-send owner notifications. Publication can replay; transport cannot.

use anyhow::{Context, Result};
use corral_core::attention::{NoticeClaim, NoticeCoverage, NoticeFact, NoticeOutcome};
use corral_core::{Store, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// A send capability returned only after this invocation committed a new source claim.
/// Deliberately not Clone: a replaying outbox reader never obtains this capability.
pub(crate) struct NewlyCommittedClaim {
    task: String,
    claim: NoticeClaim,
}

pub(crate) fn prepare(
    ctx: &Ctx,
    task: &Task,
    producer: &str,
    source: &str,
    at: Timestamp,
    coverage: Vec<NoticeCoverage>,
) -> Result<NoticeClaim> {
    let claim_id = crate::verification::digest(&serde_json::to_vec(&(
        1,
        ctx.store.root(),
        &task.id,
        producer,
        source,
    ))?);
    let batch_id = claim_id.clone();
    let mut claim = NoticeClaim {
        version: 1,
        batch_id,
        producer: producer.into(),
        source: source.into(),
        claimed_at: at,
        claim_ids: vec![claim_id],
        coverage,
        exemption_basis: None,
        outcome: None,
        pending: Vec::new(),
    };
    claim.pending.push(fact(&claim, "notification_claimed"));
    Ok(claim)
}

/// Commit a source transition and its claim together; return authority only to this caller.
pub(crate) fn commit(
    ctx: &Ctx,
    id: &str,
    change: impl FnOnce(&Store, &mut Task) -> Result<Option<NoticeClaim>>,
) -> Result<Option<NewlyCommittedClaim>> {
    let (_, claim) = ctx.store.dispatch_update(id, |store, task| {
        let Some(claim) = change(store, task)? else {
            return Ok(None);
        };
        task.attention.push_notice(claim.clone())?;
        Ok(Some(claim))
    })?;
    #[cfg(test)]
    crash_at(CrashPoint::AfterClaim)?;
    Ok(claim.map(|claim| NewlyCommittedClaim {
        task: id.into(),
        claim,
    }))
}

/// Send once, only using a capability produced by the current source transition.
pub(crate) fn notify(
    ctx: &Ctx,
    newly_committed: NewlyCommittedClaim,
    title: &str,
    body: &str,
    urgent: bool,
) -> Result<()> {
    let NewlyCommittedClaim { task, claim } = newly_committed;
    let Some(_publication) = ctx.store.try_lock(&format!("owner-notice-{task}"))? else {
        // Losing the opportunity to send is permitted; a claim is never replayed.
        return Ok(());
    };
    publish(ctx, &task, false)?;
    #[cfg(test)]
    crash_at(CrashPoint::BeforeNotify)?;
    let outcome = match ctx.herdr.notify(title, body, urgent) {
        Ok(()) => NoticeOutcome::Notified {
            at: claim.claimed_at,
        },
        Err(error) => {
            let error = format!("{error:#}");
            crate::messages::report(
                Some(&task),
                "notification",
                format!("notification for {task} failed: {error}"),
            );
            NoticeOutcome::Failed {
                at: claim.claimed_at,
                error,
            }
        }
    };
    #[cfg(test)]
    crash_at(CrashPoint::AfterNotify)?;
    let kind = match &outcome {
        NoticeOutcome::Notified { .. } => "owner_notified",
        NoticeOutcome::Failed { .. } => "owner_notify_failed",
        NoticeOutcome::NotInvoked { .. } => "notification_not_invoked",
    };
    ctx.store.update_attention(&task, |ledger| {
        let stored = ledger
            .notice_outbox
            .iter_mut()
            .find(|stored| stored.batch_id == claim.batch_id)
            .context("committed owner notice disappeared before outcome save")?;
        stored.outcome = Some(outcome);
        stored.pending.push(fact(stored, kind));
        Ok(())
    })?;
    #[cfg(test)]
    crash_at(CrashPoint::AfterOutcomeSave)?;
    publish(ctx, &task, false)
}

fn fact(claim: &NoticeClaim, kind: &str) -> NoticeFact {
    NoticeFact {
        fact_id: format!(
            "{}/{}",
            claim.batch_id,
            if kind == "notification_claimed" {
                "claimed"
            } else {
                "outcome"
            }
        ),
        kind: kind.into(),
        detail: json!({
            "batch_id": claim.batch_id,
            "producer": claim.producer,
            "source": claim.source,
            "claimed_at": claim.claimed_at,
            "claim_ids": claim.claim_ids,
            "coverage": claim.coverage,
            "exemption_basis": claim.exemption_basis,
            "outcome": claim.outcome,
        }),
    }
}

pub(crate) fn drain_task(ctx: &Ctx, id: &str) -> Result<()> {
    // Serializes publication with a sender as well as concurrent sync passes.
    let Some(_publication) = ctx.store.try_lock(&format!("owner-notice-{id}"))? else {
        return Ok(());
    };
    publish(ctx, id, true)
}

fn publish(ctx: &Ctx, id: &str, retire: bool) -> Result<()> {
    let task = ctx.store.get(id)?;
    let facts: Vec<_> = task
        .attention
        .notice_outbox
        .iter()
        .flat_map(|claim| &claim.pending)
        .cloned()
        .collect();
    let ended: Vec<_> = task
        .attention
        .notice_outbox
        .iter()
        .filter(|claim| retire && source_ended(&task, claim))
        .map(|claim| claim.batch_id.clone())
        .collect();
    if facts.is_empty() && ended.is_empty() {
        return Ok(());
    }
    for fact in &facts {
        let mut detail = fact.detail.clone();
        detail["fact_id"] = json!(fact.fact_id);
        ctx.store.log_event(Some(id), &fact.kind, detail)?;
        #[cfg(test)]
        crash_at(CrashPoint::AfterAppend)?;
    }
    #[cfg(test)]
    crash_at(CrashPoint::BeforeClear)?;
    ctx.store.update_attention(id, |ledger| {
        for claim in &mut ledger.notice_outbox {
            claim
                .pending
                .retain(|fact| !facts.iter().any(|sent| sent.fact_id == fact.fact_id));
        }
        ledger
            .notice_outbox
            .retain(|claim| !claim.pending.is_empty() || !ended.contains(&claim.batch_id));
        Ok(())
    })?;
    Ok(())
}

fn source_ended(task: &Task, claim: &NoticeClaim) -> bool {
    match claim.producer.as_str() {
        "infra_hold" => !claim.coverage.iter().any(|coverage| match coverage {
            NoticeCoverage::Exact { condition_key } => {
                task.infra_wait.as_ref().is_some_and(|wait| {
                    *condition_key == format!("{}/infra_hold/{}", task.id, wait.first_seen)
                })
            }
            NoticeCoverage::Episode { .. } => false,
        }),
        "autopilot_escalate" | "landing_blocked" => {
            !claim.coverage.iter().any(|coverage| match coverage {
                NoticeCoverage::Episode {
                    task: id,
                    run,
                    stage,
                    at,
                } => {
                    *id == task.id
                        && task.current_run().is_some_and(|current| {
                            run.as_deref() == Some(current.id.as_str())
                                && current.autopilot.as_ref().is_some_and(|pilot| {
                                    pilot.stage.as_str() == stage
                                        && pilot.at == *at
                                        && matches!(
                                            task.status,
                                            corral_core::Status::Review
                                                | corral_core::Status::NeedsMe
                                        )
                                })
                        })
                }
                NoticeCoverage::Exact { .. } => false,
            })
        }
        // Later children add their source reconcilers without losing an unknown source.
        _ => false,
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    AfterClaim,
    AfterAppend,
    BeforeClear,
    BeforeNotify,
    AfterNotify,
    AfterOutcomeSave,
}

#[cfg(test)]
thread_local! {
    static CRASH: std::cell::Cell<Option<CrashPoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn crash_at(point: CrashPoint) -> Result<()> {
    if CRASH.with(|crash| {
        if crash.get() == Some(point) {
            crash.set(None);
            true
        } else {
            false
        }
    }) {
        anyhow::bail!("injected crash at {point:?}");
    }
    Ok(())
}

/// An owning sync drains a fresh post-mutation snapshot, including terminal tasks.
pub(crate) fn drain(ctx: &Ctx) -> Result<()> {
    for task in ctx.store.list_shared()?.iter() {
        if !task.attention.notice_outbox.is_empty() && crate::session_scope::owns_task(ctx, task)? {
            drain_task(ctx, &task.id)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeSet;
    use std::os::unix::fs::PermissionsExt;

    use corral_core::{InfraWait, InfraWaitClass, Status};
    use corral_herdr::Herdr;

    use super::*;

    struct Fixture {
        _tmp: tempfile::TempDir,
        ctx: Ctx,
        task: String,
        bin: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            let executable = bin.join("herdr");
            std::fs::write(&executable, r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = --session ]; then shift 2; fi
case "$1 $2" in
  "workspace list")
    test ! -f "$dir/host-down" || exit 1
    printf '{"id":"test","result":{"workspaces":[]}}\n' ;;
  "pane list")
    test ! -f "$dir/inventory-down" || exit 1
    printf '{"id":"test","result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent_status":"idle"}]}}\n' ;;
  "notification show")
    printf 'called\n' >> "$dir/notify.log"
    test ! -f "$dir/notify-fail" || exit 1
    printf '{"id":"test","result":{}}\n' ;;
  *) printf '{"id":"test","result":{}}\n' ;;
esac
"#).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let store = Store::init(tmp.path()).unwrap();
            let task = store
                .create_initialized("Held", None, Status::Review, "", |task| {
                    task.infra_wait = Some(InfraWait {
                        version: 1,
                        class: InfraWaitClass::Contention,
                        marker: "index.lock".into(),
                        path: "/fixture/index.lock".into(),
                        first_seen: at(),
                        first_mtime: None,
                        held_at: Some(at()),
                    });
                    Ok(())
                })
                .unwrap()
                .id;
            Self {
                ctx: Ctx {
                    store,
                    herdr: Herdr::with_bin(executable, None),
                    resource_probe: crate::resources::FixedResourceProbe::arc(),
                },
                _tmp: tmp,
                task,
                bin,
            }
        }

        fn reopen(&self) -> Ctx {
            Ctx {
                store: Store::open(self.ctx.store.root()).unwrap(),
                herdr: Herdr::with_bin(self.bin.join("herdr"), None),
                resource_probe: crate::resources::FixedResourceProbe::arc(),
            }
        }

        fn claim(&self) -> Result<Option<NewlyCommittedClaim>> {
            let task = self.ctx.store.get(&self.task)?;
            let mut claim = prepare(
                &self.ctx,
                &task,
                "infra_hold",
                "R1/infra/1",
                at(),
                vec![NoticeCoverage::Exact {
                    condition_key: format!("{}/infra_hold/{}", self.task, at()),
                }],
            )?;
            claim.pending.insert(
                0,
                NoticeFact {
                    fact_id: format!("{}/infra_hold", claim.batch_id),
                    kind: "infra_hold".into(),
                    detail: json!({"run": "R1", "class": "contention",
                    "first_seen": at(), "marker": "index.lock"}),
                },
            );
            commit(&self.ctx, &self.task, |_, _| Ok(Some(claim)))
        }

        fn calls(&self) -> usize {
            std::fs::read_to_string(self.bin.join("notify.log"))
                .unwrap_or_default()
                .lines()
                .count()
        }

        fn logical(&self, kind: &str) -> BTreeSet<String> {
            self.ctx
                .store
                .events()
                .unwrap()
                .into_iter()
                .filter(|event| event.kind == kind)
                .map(|event| event.detail["fact_id"].as_str().unwrap().to_owned())
                .collect()
        }
    }

    fn at() -> Timestamp {
        "2026-10-25T06:00:00Z".parse().unwrap()
    }

    #[test]
    fn crash_windows_replay_facts_without_repeating_transport() {
        for (point, expected_calls, known) in [
            (CrashPoint::AfterClaim, 0, false),
            (CrashPoint::AfterAppend, 0, false),
            (CrashPoint::BeforeClear, 0, false),
            (CrashPoint::BeforeNotify, 0, false),
            (CrashPoint::AfterNotify, 1, false),
            (CrashPoint::AfterOutcomeSave, 1, true),
        ] {
            // Given a durable source, stop at each claim/publication/send boundary.
            let fixture = Fixture::new();
            CRASH.with(|crash| crash.set(Some(point)));
            let result = fixture
                .claim()
                .and_then(|claim| notify(&fixture.ctx, claim.unwrap(), "held", "body", true));
            assert!(result.is_err(), "{point:?}");
            assert_eq!(fixture.calls(), expected_calls, "{point:?}");
            let before = fixture.ctx.store.get(&fixture.task).unwrap();
            let claim = &before.attention.notice_outbox[0];
            assert_eq!(claim.outcome.is_some(), known, "{point:?}");
            let batch = claim.batch_id.clone();
            let coverage = claim.coverage.clone();

            // When a new board drains retained facts twice.
            let reopened = fixture.reopen();
            drain(&reopened).unwrap();
            drain(&reopened).unwrap();

            // Then publication is logically once and no crash is classified as unsent.
            assert_eq!(fixture.calls(), expected_calls, "{point:?}");
            assert_eq!(fixture.logical("notification_claimed").len(), 1);
            assert_eq!(fixture.logical("infra_hold").len(), 1);
            assert_eq!(fixture.logical("owner_notified").len(), usize::from(known));
            assert!(fixture.logical("notification_not_invoked").is_empty());
            let after = reopened.store.get(&fixture.task).unwrap();
            assert_eq!(after.updated, before.updated);
            assert_eq!(after.attention.notice_outbox[0].batch_id, batch);
            assert_eq!(after.attention.notice_outbox[0].coverage, coverage);
            assert!(after.attention.notice_outbox[0].pending.is_empty());
            assert_eq!(after.attention.notice_outbox[0].outcome.is_some(), known);
        }
    }

    #[test]
    fn returned_failure_is_recorded_and_never_retried() {
        // Given a transport that records invocation and returns failure.
        let fixture = Fixture::new();
        std::fs::write(fixture.bin.join("notify-fail"), "").unwrap();
        let claim = fixture.claim().unwrap().unwrap();

        // When it is notified, then restarted publication cannot send again.
        notify(&fixture.ctx, claim, "held", "body", true).unwrap();
        drain(&fixture.reopen()).unwrap();
        assert_eq!(fixture.calls(), 1);
        assert_eq!(fixture.logical("owner_notify_failed").len(), 1);
        assert!(fixture.logical("owner_notified").is_empty());
        let task = fixture.ctx.store.get(&fixture.task).unwrap();
        assert!(matches!(
            task.attention.notice_outbox[0].outcome,
            Some(NoticeOutcome::Failed { .. })
        ));
    }

    #[test]
    fn saved_outcome_replays_after_append_before_acknowledgement() {
        // Given a returned success saved before its event publication.
        let fixture = Fixture::new();
        let claim = fixture.claim().unwrap().unwrap();
        CRASH.with(|crash| crash.set(Some(CrashPoint::AfterOutcomeSave)));
        assert!(notify(&fixture.ctx, claim, "held", "body", true).is_err());

        // When publication appends the outcome but stops before acknowledging it.
        CRASH.with(|crash| crash.set(Some(CrashPoint::AfterAppend)));
        assert!(drain(&fixture.reopen()).is_err());
        drain(&fixture.reopen()).unwrap();

        // Then replay repeats the same fact ID, never the transport call.
        let raw: Vec<_> = fixture
            .ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "owner_notified")
            .collect();
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0].detail["fact_id"], raw[1].detail["fact_id"]);
        assert_eq!(fixture.logical("owner_notified").len(), 1);
        assert_eq!(fixture.calls(), 1);
    }

    #[test]
    fn every_owning_sync_branch_drains_claims_after_the_source_is_cleared() {
        for branch in ["normal", "host-down", "inventory-down"] {
            for known in [false, true] {
                // Given an interrupted notification and a source cleared before restart.
                let fixture = Fixture::new();
                CRASH.with(|crash| {
                    crash.set(Some(if known {
                        CrashPoint::AfterOutcomeSave
                    } else {
                        CrashPoint::AfterClaim
                    }))
                });
                let result = fixture
                    .claim()
                    .and_then(|claim| notify(&fixture.ctx, claim.unwrap(), "held", "body", true));
                assert!(result.is_err());
                fixture
                    .ctx
                    .store
                    .update(&fixture.task, |task| {
                        task.status = Status::Done;
                        Ok(())
                    })
                    .unwrap();
                if branch != "normal" {
                    std::fs::write(fixture.bin.join(branch), "").unwrap();
                }
                let reopened = fixture.reopen();

                // When the owning sync takes each main or unavailable branch.
                crate::sync_at(&reopened, at()).unwrap();

                // Then facts survive source normalization and no call is replayed.
                assert_eq!(fixture.calls(), usize::from(known), "{branch}");
                assert_eq!(fixture.logical("notification_claimed").len(), 1, "{branch}");
                assert_eq!(fixture.logical("infra_hold").len(), 1, "{branch}");
                assert_eq!(
                    fixture.logical("owner_notified").len(),
                    usize::from(known),
                    "{branch}"
                );
                assert!(
                    reopened
                        .store
                        .get(&fixture.task)
                        .unwrap()
                        .attention
                        .notice_outbox
                        .is_empty()
                );
            }
        }
    }
}

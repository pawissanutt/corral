use super::*;
use crate::agent::test_support as ts;
use crate::assistant::workflow::handoff::{self, lifecycle, transfer};

fn source() -> (ts::Fx, String, handoff::Prepared) {
    let mut f = ts::fx();
    let (session, _, expected) = handoff::tests::planning(&mut f);
    f.ctx.store.save_config().unwrap();
    let mut host = ts::bind(
        &f,
        &crate::agent::BindTarget::Assistant {
            session: session.clone(),
        },
        "source-pane",
        "source-session",
    );
    host.identity.project = String::new();
    ts::committed(&ts::hello(&f, &host));
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let mut request = handoff::tests::request();
    request.goal = handoff::Goal::PlanContinue;
    let prepared = handoff::prepare(&f.ctx.store, &session, &c, &expected, request, None).unwrap();
    (f, session, prepared)
}

#[test]
fn missing_actual_source_proof_cannot_publish_a_task_or_reassign_ownership() {
    let (f, session, prepared) = source();
    let choice = transfer::preview(&f.ctx, prepared.id(), "current source", None, true).unwrap();
    assert!(
        transfer::commit(
            &f.ctx,
            &choice,
            &choice.digest().unwrap(),
            lifecycle::Instruction::Shell {
                text: "fixture selected scope".into(),
                at: corral_core::clock::now(),
            }
        )
        .is_err()
    );
    assert!(f.ctx.store.list().unwrap().is_empty());
    let journal = handoff::read_journal(&f.ctx.store, prepared.id())
        .unwrap()
        .unwrap();
    assert!(journal.reserved && journal.source_retained && journal.source_boundary.is_none());
    assert!(
        journal
            .waiting
            .as_ref()
            .is_some_and(|reason| !reason.is_empty())
    );
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    assert_eq!(
        c.workflow.as_ref().unwrap().current.owner,
        corral_core::assistant_workflow::ScopeOwner::Local
    );
    assert!(
        crate::agent::open_generation(
            &f.ctx.store,
            &crate::agent::BindTarget::Assistant { session },
            "replacement"
        )
        .is_err()
    );
}

#[test]
fn cached_native_proof_binds_exact_host_session_and_reserved_epoch_without_requiring_host_liveness()
{
    let (f, session, prepared) = source();
    let journal = f
        .ctx
        .store
        .settle_assistant_container(&session, |c, _| {
            handoff::reserve(&f.ctx.store, &session, c, &prepared)
        })
        .unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let generation = c.state.current_generation().unwrap();
    let host = generation.bound.as_ref().unwrap();
    // This is a validator contract, not a claim that the fixture process ran C1 drain.
    let native = NativeProof {
        version: 1,
        transfer: journal.id.clone(),
        assistant: session,
        generation: generation.n,
        session_id: host.session_id.clone(),
        pid: host.pid,
        start_time: host.start_time.clone(),
        expected: reserved(&c, &journal).unwrap(),
    };
    Proof::Native {
        proof: native.clone(),
    }
    .require_current(&f.ctx.store, &c, &journal)
    .unwrap();
    let mut stale_epoch = native.clone();
    stale_epoch.expected.authority_epoch += 1;
    let mut stale_session = native.clone();
    stale_session.session_id.push_str("-other");
    let mut stale_host = native.clone();
    stale_host.start_time.push('0');
    let mut unsupported = native;
    unsupported.version += 1;
    for proof in [stale_epoch, stale_session, stale_host, unsupported] {
        assert!(
            Proof::Native { proof }
                .require_current(&f.ctx.store, &c, &journal)
                .is_err()
        );
    }
}

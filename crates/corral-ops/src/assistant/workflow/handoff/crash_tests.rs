//! Slow: publication/crash matrix over durable workflow records; owning trigger is handoff/.
use super::tests::{planning, request};
use super::*;
use crate::agent::{FAILPOINT, test_support as ts};

#[test]
fn handoff_publication_crash_matrix_reconciles_package_and_reservation_without_effect_replay() {
    for point in [
        "handoff_package_prepared",
        "handoff_preview_published",
        "handoff_reservation_staged",
    ] {
        let mut f = ts::fx();
        let (session, container, expected) = planning(&mut f);
        FAILPOINT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |p| {
                if p == point {
                    panic!("crash at {point}");
                }
            }))
        });
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let prepared = prepare(
                &f.ctx.store,
                &session,
                &container,
                &expected,
                request(),
                None,
            )
            .unwrap();
            f.ctx
                .store
                .update_assistant_container(&session, |c| {
                    reserve(&f.ctx.store, &session, c, &prepared)
                })
                .unwrap();
        }));
        FAILPOINT.with_borrow_mut(|hook| *hook = None);
        assert!(crashed.is_err());
        assert_eq!(
            f.ctx.store.get_assistant_container(&session).unwrap(),
            container
        );
        let prepared = prepare(
            &f.ctx.store,
            &session,
            &container,
            &expected,
            request(),
            None,
        )
        .unwrap();
        // Simulate committed publication with a lost acknowledgement, then reopen/read/retry.
        let journal = f
            .ctx
            .store
            .update_assistant_container(&session, |c| reserve(&f.ctx.store, &session, c, &prepared))
            .unwrap();
        let reopened = Store::open(f._tmp.path()).unwrap();
        assert_eq!(
            read_journal(&reopened, &journal.id).unwrap(),
            Some(journal.clone())
        );
        let again = reopened
            .update_assistant_container(&session, |c| reserve(&reopened, &session, c, &prepared))
            .unwrap();
        assert_eq!(again, journal);
        assert_eq!(reopened.record_rows(JOURNALS).unwrap().len(), 1);
        assert_eq!(reopened.record_rows(PACKAGES).unwrap().len(), 1);
        assert!(
            reopened.list().unwrap().is_empty(),
            "C2 cannot file or dispatch"
        );
        assert_eq!(
            reopened
                .get_assistant_container(&session)
                .unwrap()
                .workflow
                .unwrap()
                .current
                .owner,
            ScopeOwner::Local
        );
    }
}

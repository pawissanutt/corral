//! Slow: publication/crash matrix over durable workflow records; owning trigger is handoff/.
use super::tests::{planning, record_source, repo, request};
use super::*;
use crate::agent::{FAILPOINT, test_support as ts};

#[test]
fn handoff_publication_crash_matrix_reconciles_package_and_reservation_without_effect_replay() {
    for (point, occurrence) in [
        ("handoff_snapshot_captured", 1),
        ("handoff_content_published", 1),
        ("handoff_content_published", 6),
        ("handoff_restore_verified", 1),
        ("handoff_package_prepared", 1),
        ("handoff_preview_published", 1),
        ("handoff_reservation_staged", 1),
    ] {
        let mut f = ts::fx();
        let (session, container, expected) = planning(&mut f);
        let source = f._tmp.path().join("source");
        let base = repo(&source);
        record_source(&f, &session, &source, &base);
        let before = git::scan(&source, &base, "work").unwrap().state;
        let seen = std::cell::Cell::new(0);
        FAILPOINT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |p| {
                if p == point {
                    seen.set(seen.get() + 1);
                    if seen.get() == occurrence {
                        panic!("crash at {point} #{occurrence}");
                    }
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
                Some((&source, &base, "work")),
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
        assert!(
            crashed.is_err(),
            "fault boundary {point} #{occurrence} was not reached"
        );
        assert_eq!(
            f.ctx.store.get_assistant_container(&session).unwrap(),
            container
        );
        assert_eq!(git::scan(&source, &base, "work").unwrap().state, before);
        assert!(f.ctx.store.record_rows(JOURNALS).unwrap().is_empty());
        let published = usize::from(matches!(
            point,
            "handoff_preview_published" | "handoff_reservation_staged"
        ));
        assert_eq!(
            f.ctx.store.record_rows(PACKAGES).unwrap().len(),
            published,
            "interrupted capture is not a completed preview"
        );
        assert_eq!(
            f.ctx.store.record_rows(INTENTS).unwrap().len(),
            published,
            "intent lookup publishes atomically with the preview"
        );
        let prepared = prepare(
            &f.ctx.store,
            &session,
            &container,
            &expected,
            request(),
            Some((&source, &base, "work")),
        )
        .unwrap();
        // Simulate committed publication with a lost acknowledgement, then reopen/read/recover.
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
        let recovered = recover(&reopened, &journal.id).unwrap();
        let again = reopened
            .update_assistant_container(&session, |c| reserve(&reopened, &session, c, &recovered))
            .unwrap();
        assert_eq!(again, journal);
        assert_eq!(reopened.record_rows(JOURNALS).unwrap().len(), 1);
        assert_eq!(reopened.record_rows(PACKAGES).unwrap().len(), 1);
        assert_eq!(reopened.record_rows(INTENTS).unwrap().len(), 1);
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
        assert!(require_source_releasable(&reopened, &session, &source).is_err());
    }
}

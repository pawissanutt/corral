//! The same authorization retry and keyed-read contract on both implementations.

use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::*;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};
use serde_json::json;

fn backend(dir: &std::path::Path, sqlite: bool) -> Box<dyn StoreBackend> {
    if sqlite {
        Box::new(SqliteBackend::open(&dir.join(".corral"), SqliteOptions::default()).unwrap())
    } else {
        Box::new(FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new("2026-01-01T00:00:00Z".parse().unwrap())),
            Arc::new(FaultInjector::default()),
        ))
    }
}

fn operation(id: &str, from: Option<Disposition>, to: Disposition, revision: u64) -> Operation {
    Operation::Authorization(Authorization {
        id: id.into(),
        task: "T-001".into(),
        from,
        to,
        doc: json!({"revision": revision}),
        change: None,
    })
}

fn lookup(backend: &dyn StoreBackend, id: &str) -> Vec<StorageRow> {
    let page = backend
        .view()
        .unwrap()
        .storage_rows(
            &StorageQuery::Authorization { id: id.into() },
            PageReq::first(1),
        )
        .unwrap();
    assert!(page.next.is_none());
    page.items
}

#[test]
fn preparation_refresh_and_definitely_unsubmitted_retries_keep_one_authorization() {
    for sqlite in [false, true] {
        // Given an absent authorization, on each implementation.
        let dir = tempfile::tempdir().unwrap();
        let backend = backend(dir.path(), sqlite);
        let mut from = None;
        // When current authority is refreshed, refused, refreshed, and eventually submitted.
        for (index, to) in [
            Disposition::Prepared,
            Disposition::Prepared,
            Disposition::Refused,
            Disposition::Refused,
            Disposition::Prepared,
            Disposition::Refused,
            Disposition::Submitting,
            Disposition::Submitted,
        ]
        .into_iter()
        .enumerate()
        {
            let revision = u64::try_from(index).unwrap() + 1;
            backend
                .apply(operation("action", from, to, revision))
                .unwrap();
            // Then every legal transition keeps its exact refreshed document and disposition.
            let rows = lookup(backend.as_ref(), "action");
            assert!(matches!(
                rows.as_slice(),
                [StorageRow::Authorization(row)] if row.disposition == to
                    && row.doc == json!({"revision": revision})
            ));
            from = Some(to);
        }
        assert!(backend.check().unwrap().is_ok());
    }
}

#[test]
fn submitted_and_uncertain_authorizations_refuse_replay_without_a_change_entry() {
    for sqlite in [false, true] {
        for terminal in [Disposition::Submitted, Disposition::Uncertain] {
            // Given an authorization whose single effect attempt already happened.
            let dir = tempfile::tempdir().unwrap();
            let backend = backend(dir.path(), sqlite);
            backend
                .apply(operation("action", None, Disposition::Prepared, 1))
                .unwrap();
            backend
                .apply(operation(
                    "action",
                    Some(Disposition::Prepared),
                    Disposition::Submitting,
                    2,
                ))
                .unwrap();
            backend
                .apply(operation(
                    "action",
                    Some(Disposition::Submitting),
                    terminal,
                    3,
                ))
                .unwrap();
            let before = backend.view().unwrap().seq();
            let rows = lookup(backend.as_ref(), "action");
            // When a caller tries to prepare or submit the same attempt again.
            for to in [Disposition::Prepared, Disposition::Submitting] {
                assert!(matches!(
                    backend.apply(operation("action", Some(terminal), to, 4)),
                    Err(ApplyError::Precondition(_))
                ));
            }
            // Then neither its document nor the change feed moves.
            assert_eq!(lookup(backend.as_ref(), "action"), rows);
            assert_eq!(backend.view().unwrap().seq(), before);
        }
    }
}

#[test]
fn keyed_authorization_reads_exclude_unrelated_history_and_unknown_ids() {
    for sqlite in [false, true] {
        // Given many unrelated authorizations, including keys before and after the target.
        let dir = tempfile::tempdir().unwrap();
        let backend = backend(dir.path(), sqlite);
        for index in 0..100 {
            backend
                .apply(operation(
                    &format!("action-{index:03}"),
                    None,
                    Disposition::Prepared,
                    index,
                ))
                .unwrap();
        }
        // When reading exactly one identity through a pinned view.
        let view = backend.view().unwrap();
        let page = view
            .storage_rows(
                &StorageQuery::Authorization {
                    id: "action-072".into(),
                },
                PageReq::first(1),
            )
            .unwrap();
        // Then no neighbor or continuation is returned; an absent identity is absent.
        assert!(matches!(
            page.items.as_slice(),
            [StorageRow::Authorization(row)] if row.id == "action-072"
                && row.doc == json!({"revision": 72})
        ));
        assert!(page.next.is_none());
        assert!(lookup(backend.as_ref(), "action-072-missing").is_empty());
    }
}

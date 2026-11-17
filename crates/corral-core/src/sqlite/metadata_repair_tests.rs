use super::*;

#[test]
fn metadata_repair_preserves_rows_rebuilds_counts_and_rolls_back_failed_audit() {
    let root = tempfile::tempdir().unwrap();
    let faults = Arc::new(FaultInjector::default());
    let backend = SqliteBackend::open(
        root.path(),
        SqliteOptions {
            faults: faults.clone(),
            ..SqliteOptions::default()
        },
    )
    .unwrap();
    let scope = ChangeScope::Record {
        area: "gate-triage".into(),
        key: "T-855".into(),
    };
    {
        // Reproduce the external PM writer: records, change and scope commit together,
        // but the writer bypasses apply and omits the meta.change_seq update.
        let mut conn = backend.writer();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "INSERT INTO records(area, key, version, doc, updated) \
             VALUES ('gate-triage', 'T-855', 1, '{\"decision\":\"preserve\"}', ?1)",
            params!["2026-11-16T17:52:21.250470Z"],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO changes(at, op) VALUES (?1, 'put_records')",
            params!["2026-11-16T17:52:21.250470Z"],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO change_scopes(seq, kind, key) VALUES (?1, 'record', ?2)",
            params![tx.last_insert_rowid(), codec::json(&scope).unwrap()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO tasks(num, id, rev, title, status, project, priority, tracker, design, \
             queued, created, updated, spec_rev, spec, state, body, history, doc_version) \
             VALUES (898, 'T-898', 1, 'preserve done task', 'done', 'corral', 'medium', 0, 0, \
             0, ?1, ?1, '', '{}', '{}', '', '{}', 1)",
            params!["2026-11-16T00:29:47.363000000Z"],
        )
        .unwrap();
        // A second external writer changed the task from ready to done, but left the
        // derived ready count behind. The repair must retain the authoritative done state.
        tx.execute(
            "INSERT INTO status_counts(status, project, n) VALUES ('ready', 'corral', 1)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let record_query = RecordQuery {
        area: Some("gate-triage".into()),
        key: Some("T-855".into()),
    };
    let audit_query = EventQuery {
        kinds: vec!["storage_metadata_repaired".into()],
        ..EventQuery::default()
    };
    let stale_counts = vec![StatusCount {
        status: "ready".into(),
        project: "corral".into(),
        n: 1,
    }];
    let correct_counts = vec![StatusCount {
        status: "done".into(),
        project: "corral".into(),
        n: 1,
    }];
    let before = backend.view().unwrap();
    assert_eq!(before.seq(), ChangeSeq(0));
    let original_records = before
        .records(&record_query, PageReq::first(10))
        .unwrap()
        .items;
    let original_changes = before.changes(ChangeSeq(0), 10).unwrap().entries;
    assert_eq!(original_records.len(), 1);
    assert_eq!(original_changes.len(), 1);
    assert_eq!(original_changes[0].scopes, vec![scope]);
    drop(before);
    assert!(
        backend
            .apply(Operation::PutRecords(PutRecords {
                writes: Vec::new(),
                events: Vec::new(),
                reservations: Vec::new(),
            }))
            .is_err()
    );

    // The audit fails after both its event and change entry were written. The provisional
    // counter advance, rebuilt counts, audit event, scopes and audit change all roll back.
    faults.arm(2);
    let error = backend.repair_metadata().unwrap_err();
    assert!(format!("{error:#}").contains("change entry"));
    assert_eq!(backend.change_seq().unwrap(), 0);
    let failed = backend.view().unwrap();
    assert_eq!(failed.count_events(&audit_query).unwrap(), 0);
    assert_eq!(
        failed.changes(ChangeSeq(0), 10).unwrap().entries,
        original_changes
    );
    assert_eq!(
        failed
            .aggregates(&[AggregateName::StatusCounts])
            .unwrap()
            .status_counts,
        stale_counts
    );
    drop(failed);

    assert_eq!(
        backend.repair_metadata().unwrap(),
        MetadataRepair {
            previous_change_seq: 0,
            change_seq: 2,
            status_counts_rebuilt: true,
        }
    );
    assert!(backend.check().unwrap().problems.is_empty());
    let repaired = backend.view().unwrap();
    assert_eq!(repaired.seq(), ChangeSeq(2));
    assert_eq!(
        repaired
            .records(&record_query, PageReq::first(10))
            .unwrap()
            .items,
        original_records
    );
    let changes = repaired.changes(ChangeSeq(0), 10).unwrap().entries;
    assert_eq!(&changes[..1], original_changes.as_slice());
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1].seq, ChangeSeq(2));
    assert!(changes[1].scopes.contains(&ChangeScope::Aggregate {
        name: AggregateName::StatusCounts,
    }));
    assert_eq!(
        repaired
            .aggregates(&[AggregateName::StatusCounts])
            .unwrap()
            .status_counts,
        correct_counts
    );
    let audit = repaired.events(&audit_query, PageReq::first(10)).unwrap();
    assert_eq!(audit.items.len(), 1);
    assert_eq!(
        audit.items[0].event.detail,
        serde_json::json!({
            "old_change_seq": 0,
            "max_change_seq": 1,
            "new_change_seq": 2,
            "status_counts_rebuilt": true,
            "status_counts_repair": {
                "before": stale_counts,
                "after": correct_counts,
            },
        })
    );
    drop(repaired);
    assert_eq!(
        backend.repair_metadata().unwrap(),
        MetadataRepair {
            previous_change_seq: 2,
            change_seq: 2,
            status_counts_rebuilt: false,
        }
    );
    assert_eq!(
        backend.view().unwrap().count_events(&audit_query).unwrap(),
        1
    );

    let committed = backend
        .apply(Operation::PutRecords(PutRecords {
            writes: vec![RecordWrite {
                area: "gate-triage".into(),
                key: "next".into(),
                expected: None,
                doc: Some(serde_json::json!({"decision": "next"})),
                updated: crate::clock::now(),
            }],
            events: Vec::new(),
            reservations: Vec::new(),
        }))
        .unwrap();
    assert_eq!(committed.seq, ChangeSeq(3));

    {
        let conn = backend.writer();
        conn.execute("UPDATE meta SET value = '2' WHERE key = 'change_seq'", [])
            .unwrap();
        conn.execute("UPDATE status_counts SET status = 'ready'", [])
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        conn.execute(
            "INSERT INTO change_scopes(seq, kind, key) VALUES (999, 'record', ?1)",
            params![
                codec::json(&ChangeScope::Record {
                    area: "orphan".into(),
                    key: "orphan".into(),
                })
                .unwrap()
            ],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    }
    let error = backend.repair_metadata().unwrap_err();
    assert!(format!("{error:#}").contains("foreign key"));
    assert_eq!(backend.change_seq().unwrap(), 2);
    assert_eq!(
        backend
            .view()
            .unwrap()
            .aggregates(&[AggregateName::StatusCounts])
            .unwrap()
            .status_counts,
        stale_counts
    );
    assert_eq!(
        backend.view().unwrap().count_events(&audit_query).unwrap(),
        1
    );
    {
        let conn = backend.writer();
        conn.execute("DELETE FROM change_scopes WHERE seq = 999", [])
            .unwrap();
        conn.execute("UPDATE meta SET value = '3' WHERE key = 'change_seq'", [])
            .unwrap();
    }
    // Count-only drift still receives an audit/new sequence, even with a correct counter.
    assert_eq!(
        backend.repair_metadata().unwrap(),
        MetadataRepair {
            previous_change_seq: 3,
            change_seq: 4,
            status_counts_rebuilt: true,
        }
    );
    assert!(backend.check().unwrap().problems.is_empty());
    let status: String = backend
        .writer()
        .query_row("SELECT status FROM tasks WHERE num = 898", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "done");
    backend.prune_changes_through(ChangeSeq(4)).unwrap();
    assert_eq!(
        backend.repair_metadata().unwrap(),
        MetadataRepair {
            previous_change_seq: 4,
            change_seq: 4,
            status_counts_rebuilt: false,
        }
    );
    assert_eq!(
        backend.view().unwrap().count_events(&audit_query).unwrap(),
        2
    );
}

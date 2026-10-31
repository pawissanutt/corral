//! Supplemental attention selection never scans settled terminal task heads.
mod store_support;
use corral_core::attention::{AttentionFact, SourceNoticeRef};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{AttentionRecord, ConditionKey, Status};
use serde_json::json;

fn record(resolved: bool) -> AttentionRecord {
    serde_json::from_value(json!({"kind":"question_open","responsible":"pm","since":store_support::at(0),
        "since_source":"discovered","first_recorded":store_support::at(0),"opened_at":store_support::at(0),
        "resolved_at":resolved.then(||store_support::at(1))})).unwrap()
}
fn ids(store: &corral_core::Store) -> Vec<String> {
    store
        .attention_input_heads()
        .unwrap()
        .into_iter()
        .map(|head| head.id.clone())
        .collect()
}

#[test]
fn owed_attention_and_terminal_slots_are_indexed_with_constant_work_and_atomic_retirement() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        store.fixture_pin_now(store_support::at(0)).unwrap();
        let mut facts = store_support::rich(&store, 1, 0, 0, 0);
        facts.status = Status::Done;
        facts.runs[0].gate = Some(
            serde_json::from_value(
                json!({"sha":"abc","at":store_support::at(0),"result":"running"}),
            )
            .unwrap(),
        );
        facts.attention.outbox.push(AttentionFact {
            key: ConditionKey("condition".into()),
            fact_id: "fact".into(),
            kind: "attention_opened".into(),
            detail: json!({}),
        });
        store.fixture_put_task(&facts).unwrap();
        let mut source = store.create("Source", None, Status::Dropped, "").unwrap();
        source.attention.source_notices.push(SourceNoticeRef {
            area: "availability".into(),
            key: "episode".into(),
            claim_id: "claim".into(),
        });
        store.fixture_put_task(&source).unwrap();
        let mut unresolved = store.create("Unresolved", None, Status::Done, "").unwrap();
        unresolved
            .attention
            .records
            .insert(ConditionKey("old".into()), record(false));
        store.fixture_put_task(&unresolved).unwrap();
        let mut settled = store
            .create("Retained resolved", None, Status::Done, "")
            .unwrap();
        settled
            .attention
            .records
            .insert(ConditionKey("old".into()), record(true));
        store.fixture_put_task(&settled).unwrap();
        for _ in 0..2 * scale {
            let mut unrelated = store.create("Settled", None, Status::Done, "").unwrap();
            unrelated.attention.records = settled.attention.records.clone();
            store.fixture_put_task(&unrelated).unwrap();
        }
        let frozen = store.read_only_snapshot().unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            ids(&frozen),
            vec![facts.id.clone(), source.id.clone(), unresolved.id.clone()]
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        let failed = store.update_attention_head(&source.id, |ledger| {
            ledger.source_notices.clear();
            anyhow::bail!("refused")
        });
        assert!(failed.is_err());
        assert_eq!(
            ids(&store),
            vec![facts.id.clone(), source.id.clone(), unresolved.id.clone()]
        );
        store
            .update_attention_head(&source.id, |ledger| {
                ledger.source_notices.clear();
                Ok(())
            })
            .unwrap();
        store
            .update_attention_head(&unresolved.id, |ledger| {
                ledger
                    .records
                    .get_mut(&ConditionKey("old".into()))
                    .unwrap()
                    .resolved_at = Some(store_support::at(1));
                Ok(())
            })
            .unwrap();
        store
            .update_attention_head(&facts.id, |ledger| {
                ledger.outbox.clear();
                Ok(())
            })
            .unwrap();
        // A terminal gate alone still owns an input after its event fact was published.
        assert_eq!(ids(&store), vec![facts.id.clone()]);
        store
            .update_head(&facts.id, |head| {
                head.current_run_mut()
                    .unwrap()
                    .gate
                    .as_mut()
                    .unwrap()
                    .result = corral_core::GateResult::Pass;
                Ok(())
            })
            .unwrap();
        assert!(ids(&store).is_empty());
        // Both documents and the selector are pinned; retirement cannot alter this pass.
        assert_eq!(
            ids(&frozen),
            vec![facts.id.clone(), source.id.clone(), unresolved.id.clone()]
        );
        // The full write twin restores and clears exactly the same derived marker.
        store
            .update(&settled.id, |task| {
                task.attention
                    .records
                    .get_mut(&ConditionKey("old".into()))
                    .unwrap()
                    .resolved_at = None;
                Ok(())
            })
            .unwrap();
        assert_eq!(ids(&store), vec![settled.id.clone()]);
        store
            .update(&settled.id, |task| {
                task.attention
                    .records
                    .get_mut(&ConditionKey("old".into()))
                    .unwrap()
                    .resolved_at = Some(store_support::at(1));
                Ok(())
            })
            .unwrap();
        assert!(ids(&store).is_empty());
    }
    assert_eq!(measured[0], measured[1]);
}

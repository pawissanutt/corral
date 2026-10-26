use super::*;

#[test]
fn separate_overflow_episodes_survive_open_and_resolution_publication_crashes() {
    for point in [
        CrashPoint::AfterTransition,
        CrashPoint::AfterAppend,
        CrashPoint::BeforeClear,
    ] {
        // Given two overflow episodes with a stop at each opening and resolution boundary.
        let fx = Fx::new();
        let task = fx.questions(33);
        let original = task
            .current_run()
            .unwrap()
            .agent_host
            .as_ref()
            .unwrap()
            .questions
            .clone();
        let mut overflow_keys = BTreeSet::new();
        for (revision, start) in [(1, 0), (2, 1803)] {
            if revision == 2 {
                fx.ctx
                    .store
                    .update(&task.id, |task| {
                        let host = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
                        host.questions = original.clone();
                        for question in &mut host.questions {
                            question.revision = revision;
                            question.asked.at = at(start);
                        }
                        Ok(())
                    })
                    .unwrap();
            }
            CRASH.with(|crash| crash.set(Some(point)));
            assert!(
                reconcile(&fx.ctx, at(start), Some(true)).is_err(),
                "{point:?}"
            );
            let stopped = fx.ctx.store.get(&task.id).unwrap();
            let key = stopped
                .attention
                .records
                .iter()
                .find(|(_, record)| record.kind == "ledger_overflow")
                .unwrap()
                .0
                .clone();
            assert!(overflow_keys.insert(key.clone()), "{point:?}");
            reconcile(&fx.reopen(), at(start + 1), Some(true)).unwrap();
            assert!(
                fx.ctx
                    .store
                    .get(&task.id)
                    .unwrap()
                    .attention
                    .records
                    .contains_key(&key)
            );
            for ordinary in fx
                .ctx
                .store
                .get(&task.id)
                .unwrap()
                .attention
                .records
                .into_keys()
                .filter(|candidate| *candidate != key)
            {
                attend(
                    &fx.ctx,
                    &ordinary.0,
                    "Investigating",
                    86400,
                    "pause",
                    at(start + 1),
                )
                .unwrap();
            }
            // When its owner deadline passes, then one new physical batch belongs to this episode.
            reconcile(&fx.reopen(), at(start + 1800), Some(true)).unwrap();
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.current_run_mut()
                        .unwrap()
                        .agent_host
                        .as_mut()
                        .unwrap()
                        .questions
                        .clear();
                    Ok(())
                })
                .unwrap();
            CRASH.with(|crash| crash.set(Some(point)));
            assert!(
                reconcile(&fx.reopen(), at(start + 1801), Some(true)).is_err(),
                "{point:?}"
            );
            let stopped = fx.ctx.store.get(&task.id).unwrap();
            assert!(stopped.attention.records.contains_key(&key), "{point:?}");
            reconcile(&fx.reopen(), at(start + 1802), Some(true)).unwrap();
            assert!(
                fx.ctx
                    .store
                    .get(&task.id)
                    .unwrap()
                    .attention
                    .records
                    .is_empty(),
                "{point:?}"
            );
        }
        // Then replay deduplicates within an episode, never between different episodes.
        assert_eq!(fx.calls(), 2, "{point:?}");
        let events = fx.ctx.store.events().unwrap();
        let batches: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "owner_notified")
            .collect();
        assert_eq!(batches.len(), 2, "{point:?}");
        assert_ne!(
            batches[0].detail["batch_id"], batches[1].detail["batch_id"],
            "{point:?}"
        );
        for kind in ["attention_opened", "attention_resolved"] {
            let facts: BTreeSet<_> = events
                .iter()
                .filter(|event| {
                    event.kind == kind
                        && overflow_keys.iter().any(|key| event.detail["key"] == key.0)
                })
                .map(|event| event.detail["fact_id"].as_str().unwrap())
                .collect();
            assert_eq!(facts.len(), 2, "{kind} {point:?}");
        }
    }
}

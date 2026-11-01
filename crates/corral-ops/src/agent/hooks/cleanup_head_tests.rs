//! Cleanup removes exactly unsettled coding runtimes; retained settled history/sessions stay cold.
use super::*;
use crate::agent::native_test_support::data;
use corral_core::inbox::Recipient;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn runtime_cleanup_is_indexed_ordered_and_stops_after_settlement_without_touching_sessions() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        task.status = Status::Done;
        for run in &mut task.runs {
            run.harness = Some(corral_core::PI_NATIVE_HARNESS.into());
            run.inbox.settled = Some(data::at(0));
            for child in &mut run.review_runs {
                child.harness = Some(corral_core::PI_NATIVE_HARNESS.into());
                child.inbox.settled = Some(data::at(0));
            }
        }
        let run = &mut task.runs[0];
        run.inbox.settled = None;
        run.review_runs[0].inbox.settled = None;
        let recipients = [
            Recipient::Run {
                task: task.id.clone(),
                run: run.id.clone(),
            },
            Recipient::ReviewRun {
                task: task.id.clone(),
                run: run.id.clone(),
                review: run.review_runs[0].id.clone(),
            },
        ];
        let settled = Recipient::Run {
            task: task.id.clone(),
            run: task.runs[1].id.clone(),
        };
        store.fixture_put_task(&task).unwrap();
        let ctx = Ctx::from_store(store);
        let runtime = dir.path().join("runtime");
        let paths: Vec<_> = recipients
            .iter()
            .map(|recipient| {
                crate::agent::launch::runtime_dir_in(&runtime, &ctx.store, recipient).unwrap()
            })
            .collect();
        let keep = crate::agent::launch::runtime_dir_in(&runtime, &ctx.store, &settled).unwrap();
        for path in paths.iter().chain([&keep]) {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("marker"), "runtime").unwrap();
        }
        let session = corral_core::agent::paths::container_sessions_dir(
            &ctx.store,
            &ContainerRef::Worker {
                task: task.id.clone(),
                run: task.runs[0].id.clone(),
            },
        );
        std::fs::create_dir_all(&session).unwrap();
        let session = session.join("session.jsonl");
        std::fs::write(&session, "keep transcript\n").unwrap();
        super::super::take_consulted();
        let hot = HotScope::enter();
        let before = work::snapshot();
        let head = ctx.store.head(&task.id).unwrap();
        assert_eq!(run_end_in_ref(&ctx, &head, &runtime).unwrap(), paths);
        let after = work::snapshot();
        let mut counts = Counter::ALL.map(|c| after.total(c) - before.total(c));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        assert_eq!(
            super::super::take_consulted()
                .iter()
                .filter(|seam| **seam == "cleanup")
                .count(),
            2
        );
        assert!(keep.join("marker").exists());
        assert_eq!(
            std::fs::read_to_string(&session).unwrap(),
            "keep transcript\n"
        );
        // Fixture the owning sync's settlement; a settled runtime must never be probed/deleted.
        ctx.store
            .update(&task.id, |task| {
                task.runs[0].inbox.settled = Some(data::at(1));
                task.runs[0].review_runs[0].inbox.settled = Some(data::at(1));
                Ok(())
            })
            .unwrap();
        for path in &paths {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("keep-settled"), "keep").unwrap();
        }
        super::super::take_consulted();
        let hot = HotScope::enter();
        let before = work::snapshot();
        let head = ctx.store.head(&task.id).unwrap();
        assert!(run_end_in_ref(&ctx, &head, &runtime).unwrap().is_empty());
        let after = work::snapshot();
        for (n, c) in Counter::ALL.into_iter().enumerate() {
            counts[n] += after.total(c) - before.total(c);
        }
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        assert!(!super::super::take_consulted().contains(&"cleanup"));
        for path in &paths {
            assert_eq!(
                std::fs::read_to_string(path.join("keep-settled")).unwrap(),
                "keep"
            );
        }
        measured.push(counts);
    }
    assert_eq!(measured[0], measured[1]);
}

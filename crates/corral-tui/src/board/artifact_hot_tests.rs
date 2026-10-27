use std::collections::BTreeSet;
use std::sync::Arc;

use corral_core::work::{self, Counter, HotScope};
use corral_core::{Artifact, ArtifactKind, CardField, Status};

use super::tests::{run, seeded};
use super::*;

fn artifact(run: &str, kind: ArtifactKind, path: String) -> Artifact {
    serde_json::from_value(serde_json::json!({
        "run": run, "kind": kind, "title": "History", "path": path,
        "source": "/tmp/history.log", "sha256": "0".repeat(64), "bytes": 1,
        "added": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn artifact_growth_keeps_production_frame_and_pm_work_constant_and_cards_equal_full_oracle() {
    let mut frames = Vec::new();
    let mut pm = Vec::new();
    let mut actions = Vec::new();
    let mut fx = seeded();
    let probe = Arc::new(corral_ops::resources::PinnedReadingsProbe::admitting());
    fx.ctx.resource_probe = probe.clone();
    fx.ctx.store.config.ui.done_max = 1;
    let initial = fx.ctx.store.get("T-006").unwrap();
    let mut reviewed = BTreeSet::from([initial.artifacts[1].path.clone()]);
    let mut old_count = 0;
    for factor in [1, 10] {
        // Given current-run growth, historical run growth and unrelated reviewed terminal tasks.
        let mut active = initial.clone();
        for n in 0..4 * factor {
            active.artifacts.push(artifact(
                "R1",
                ArtifactKind::Log,
                format!("artifacts/T-006/R1/{n:04}.log"),
            ));
            active.runs.insert(0, run(&format!("old-{n:04}"), "worker"));
        }
        fx.ctx.store.fixture_put_task(&active).unwrap();
        // Include a current task's shared Diff/Summary path to check duplicate registration semantics.
        for n in old_count..4 * factor {
            let mut old = fx
                .ctx
                .store
                .create("Old", None, Status::Dropped, "")
                .unwrap();
            let path = format!("artifacts/{}/R0/{n:04}.md", old.id);
            old.artifacts
                .push(artifact("R0", ArtifactKind::Report, path.clone()));
            fx.ctx.store.fixture_put_task(&old).unwrap();
            reviewed.insert(path);
        }
        old_count = 4 * factor;
        fx.ctx
            .store
            .save_record("reviewed-artifacts", "set", &reviewed)
            .unwrap();
        let full = fx.ctx.store.get("T-006").unwrap();
        let before = work::snapshot();
        let samples = probe.samples();
        // When the production frame entry point loads the board, with no preparation shortcut.
        let snapshot = snapshot::BoardSnapshot::load(&fx.ctx).unwrap();
        let report = work::snapshot().since(&before);
        assert_eq!(probe.samples() - samples, 1);
        assert_eq!(report.followup_calls("FU-3", "sample"), 1);
        assert!(snapshot.resources.reason.is_none());
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        frames.push(report);

        // Then visible artifact scalars and every density equal the full-task renderer.
        let mut board = fx.board();
        board.apply_snapshot(snapshot);
        let head = board.heads.get(&full.id).unwrap();
        let shell = board.tasks.iter().find(|t| t.id == full.id).unwrap();
        let expected_unread = full
            .artifacts
            .iter()
            .filter(|a| {
                a.tier() == corral_core::ArtifactTier::Keep
                    && a.purged_at.is_none()
                    && !reviewed.contains(&a.path)
            })
            .count();
        assert_eq!(
            board.unreviewed.get(&full.id).copied().unwrap_or(0),
            expected_unread
        );
        for density in [
            corral_core::Density::Comfortable,
            corral_core::Density::Rich,
            corral_core::Density::Compact,
        ] {
            let mut config = fx.ctx.store.config.clone();
            config.board.card.density = density;
            config.board.card.fields = CardField::ALL.iter().map(|f| f.key().to_owned()).collect();
            for width in [12, 40, 100] {
                let full_lines = cards::card_lines(
                    &full,
                    &[],
                    &config,
                    width,
                    false,
                    fx.now.get(),
                    CardExtra::default(),
                );
                let head_lines = cards::card_lines(
                    shell,
                    &[],
                    &config,
                    width,
                    false,
                    fx.now.get(),
                    CardExtra {
                        head: Some(head),
                        obligations: board.obligations.get(&full.id),
                        ..Default::default()
                    },
                );
                assert_eq!(head_lines, full_lines, "{density:?}, {width}");
            }
        }
        let before = work::snapshot();
        let samples = probe.samples();
        let action;
        {
            let _hot = HotScope::enter();
            action = corral_ops::pm_next(&fx.ctx).unwrap();
        }
        let report = work::snapshot().since(&before);
        assert!(probe.samples() > samples);
        assert_eq!(
            report.followup_calls("FU-3", "sample"),
            probe.samples() - samples
        );
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        pm.push(report);
        actions.push(serde_json::to_value(action).unwrap());
    }
    for reports in [&frames, &pm] {
        for counter in Counter::ALL {
            assert_eq!(
                reports[0].get(counter),
                reports[1].get(counter),
                "{counter:?}"
            );
        }
    }
    assert_eq!(actions[0], actions[1]);
}

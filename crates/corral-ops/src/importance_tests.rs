use corral_core::{Artifact, ArtifactKind, ArtifactTier, RetentionAction, Status, Store};
use tempfile::TempDir;

use super::*;
use crate::{
    Ctx, Preview, add_artifact, artifact_document, artifact_gc, artifact_preview, set_artifact_tier,
};

struct Fx {
    tmp: TempDir,
    ctx: Ctx,
    task: String,
}

fn fx(action: RetentionAction) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.artifacts.retention.action = action;
    store.config.artifacts.retention.larger_than = Some("1B".into());
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let task = ctx
        .store
        .create("Importance", None, Status::Done, "")
        .unwrap()
        .id;
    Fx { tmp, ctx, task }
}

impl Fx {
    fn add(&self, name: &str, body: &str, kind: ArtifactKind, title: &str) -> Artifact {
        let path = self.tmp.path().join(name);
        std::fs::write(&path, body).unwrap();
        add_artifact(&self.ctx, &self.task, &path, Some(kind), Some(title)).unwrap()
    }

    fn current(&self, a: &Artifact) -> Artifact {
        self.ctx
            .store
            .get(&self.task)
            .unwrap()
            .artifacts
            .into_iter()
            .find(|x| x.path == a.path)
            .unwrap()
    }

    fn tier(&self, a: &Artifact, tier: ArtifactTier) {
        let reference = corral_core::PinRef::of_artifact(&self.task, a).to_string();
        set_artifact_tier(&self.ctx, &reference, tier).unwrap();
    }

    fn all(&self) -> Vec<Artifact> {
        self.ctx.store.get(&self.task).unwrap().artifacts
    }
}

fn titles(v: &[Artifact]) -> Vec<&str> {
    v.iter().map(|a| a.title.as_str()).collect()
}

#[test]
fn order_follows_stored_tiers_overrides_and_report_before_summary() {
    let f = fx(RetentionAction::Keep);
    f.add("a.log", "log\n", ArtifactKind::Log, "log");
    let evidence = f.add("b.json", "{}\n", ArtifactKind::Evidence, "evidence");
    f.add("c.md", "# s\n", ArtifactKind::Summary, "summary");
    f.add("d.md", "# r\n", ArtifactKind::Report, "report");
    let promoted = f.add("e.txt", "x\n", ArtifactKind::Other, "promoted");
    f.tier(&promoted, ArtifactTier::Keep);
    f.tier(&evidence, ArtifactTier::Transient);
    f.ctx
        .store
        .update(&f.task, |task| {
            // Wall-clock reads can tie or move backward; this assertion needs
            // evidence to be strictly newer than the log.
            for (artifact, second) in task.artifacts.iter_mut().zip(0..) {
                artifact.added = jiff::Timestamp::from_second(second).unwrap();
            }
            Ok(())
        })
        .unwrap();
    let mut v = f.all();
    order_artifacts(&mut v);
    assert_eq!(
        titles(&v),
        ["report", "summary", "promoted", "evidence", "log"]
    );
}

#[test]
fn equal_importance_preserves_stored_order() {
    let f = fx(RetentionAction::Keep);
    f.add("a.log", "log\n", ArtifactKind::Log, "log");
    let evidence = f.add("b.json", "{}\n", ArtifactKind::Evidence, "evidence");
    f.tier(&evidence, ArtifactTier::Transient);
    f.ctx
        .store
        .update(&f.task, |task| {
            for artifact in &mut task.artifacts {
                artifact.added = jiff::Timestamp::UNIX_EPOCH;
            }
            Ok(())
        })
        .unwrap();
    let mut v = f.all();
    assert_eq!(cmp_importance(&v[0], &v[1]), std::cmp::Ordering::Equal);
    order_artifacts(&mut v);
    assert_eq!(titles(&v), ["log", "evidence"]);
}

#[test]
fn superseded_diffs_sort_after_the_retained_one() {
    let f = fx(RetentionAction::Keep);
    let first = f.add("1.diff", "+1\n", ArtifactKind::Diff, "first diff");
    let kept = f.add("2.diff", "+2\n", ArtifactKind::Diff, "kept diff");
    f.tier(&kept, ArtifactTier::Keep);
    let latest = f.add("3.diff", "+3\n", ArtifactKind::Diff, "latest diff");
    assert_eq!(
        f.current(&first).tier(),
        ArtifactTier::Transient,
        "superseded"
    );
    assert_eq!(
        f.current(&kept).tier(),
        ArtifactTier::Keep,
        "explicit tier wins"
    );
    assert_eq!(latest.tier(), ArtifactTier::Normal, "retained");
    let mut v = f.all();
    order_artifacts(&mut v);
    assert_eq!(titles(&v), ["kept diff", "latest diff", "first diff"]);
}

#[test]
fn excerpt_decodes_compressed_snapshots_and_skips_purged_and_binary_ones() {
    let f = fx(RetentionAction::Compress);
    let body = format!("# Compressed heading\n{}", "filler line\n".repeat(200));
    let report = f.add("r.md", &body, ArtifactKind::Report, "report");
    f.tier(&report, ArtifactTier::Normal);
    artifact_gc(&f.ctx, false).unwrap();
    let report = f.current(&report);
    assert!(report.compressed_bytes.is_some(), "compressed on disk");
    assert_eq!(
        excerpt(&f.ctx, &report).as_deref(),
        Some("Compressed heading")
    );

    let image = f.add(
        "shot.png",
        "\u{0}PNG\u{0}",
        ArtifactKind::Screenshot,
        "shot",
    );
    assert_eq!(
        excerpt(&f.ctx, &image),
        None,
        "binary snapshots have no excerpt"
    );

    let g = fx(RetentionAction::Purge);
    let old = g.add("old.md", "# Gone heading\n", ArtifactKind::Report, "old");
    g.tier(&old, ArtifactTier::Normal);
    artifact_gc(&g.ctx, false).unwrap();
    let old = g.current(&old);
    assert!(old.purged_at.is_some());
    assert_eq!(
        excerpt(&g.ctx, &old),
        None,
        "the purge stub is not an excerpt"
    );
}

#[test]
fn document_returns_the_whole_decoded_log_while_preview_truncates() {
    let f = fx(RetentionAction::Compress);
    let log: String = (1..=2500)
        .map(|i| format!("line {i}\n"))
        .chain(["error: final failure\n".to_string()])
        .collect();
    let a = f.add("long.log", &log, ArtifactKind::Log, "long log");
    artifact_gc(&f.ctx, false).unwrap();
    let a = f.current(&a);
    assert!(a.compressed_bytes.is_some());
    let Preview::Text(preview) = artifact_preview(&f.ctx, &a).unwrap() else {
        panic!("text preview");
    };
    assert!(!preview.contains("final failure") && preview.contains("truncated"));
    let Preview::Text(document) = artifact_document(&f.ctx, &a).unwrap() else {
        panic!("text document");
    };
    assert_eq!(document, log, "the whole decoded text, never cut");
}

#[test]
fn lead_artifact_is_the_newest_report_else_the_summary() {
    let f = fx(RetentionAction::Keep);
    f.add("s.md", "# s\n", ArtifactKind::Summary, "summary");
    f.add("l.log", "x\n", ArtifactKind::Log, "log");
    let v = f.all();
    assert_eq!(
        lead_artifact(&v).map(|i| v[i].title.as_str()),
        Some("summary")
    );
    f.add("r.md", "# r\n", ArtifactKind::Report, "report");
    let v = f.all();
    assert_eq!(
        lead_artifact(&v).map(|i| v[i].title.as_str()),
        Some("report")
    );
    assert_eq!(lead_artifact(&v[1..2]), None);
}

#[test]
fn reviewed_marks_toggle_and_persist() {
    let f = fx(RetentionAction::Keep);
    let a = f.add("r.md", "# r\n", ArtifactKind::Report, "report");
    assert!(reviewed_set(&f.ctx).is_empty());
    assert!(toggle_reviewed(&f.ctx, &a).unwrap());
    assert!(reviewed_set(&f.ctx).contains(&a.path));
    assert!(!toggle_reviewed(&f.ctx, &a).unwrap());
    assert!(reviewed_set(&f.ctx).is_empty());
}

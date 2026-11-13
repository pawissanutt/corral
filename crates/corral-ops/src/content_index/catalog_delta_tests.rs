use super::Catalog;
use crate::{ContentSearch, Ctx, search_content};
use corral_core::{Status, Store};

fn drain(ctx: &Ctx) {
    for _ in 0..128 {
        if super::refresh::advance_batch(ctx).unwrap() {
            return;
        }
    }
    panic!("tiny delta fixture did not publish within 128 batches");
}

fn hits(ctx: &Ctx, query: &str, reindex: bool) -> serde_json::Value {
    let options = ContentSearch {
        scope: "specs".into(),
        reindex,
        ..ContentSearch::default()
    };
    serde_json::to_value(search_content(ctx, query, &options).unwrap()).unwrap()
}

#[test]
fn changed_source_after_publication_uses_canonical_text_not_stale_terms() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("delta fixture", None, Status::Ready, "obsoletequartz")
        .unwrap();
    drain(&ctx);
    ctx.store
        .update(&task.id, |task| {
            task.body = "canonicalzircon".into();
            Ok(())
        })
        .unwrap();

    assert!(Catalog::open(&ctx, false).unwrap().is_some());
    let actual = hits(&ctx, "canonicalzircon", false);
    assert_eq!(actual.as_array().unwrap().len(), 1);
    assert!(
        hits(&ctx, "obsoletequartz", false)
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(actual, hits(&ctx, "canonicalzircon", true));
    assert!(
        hits(&ctx, "obsoletequartz", true)
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn deleted_artifact_after_publication_is_absent_from_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("deletion fixture", None, Status::Ready, "retainedzircon")
        .unwrap();
    let source = dir.path().join("evidence.log");
    std::fs::write(&source, "deletedquartz\n").unwrap();
    crate::add_artifact(&ctx, &task.id, &source, None, None).unwrap();
    drain(&ctx);
    ctx.store
        .update(&task.id, |task| {
            task.artifacts.clear();
            Ok(())
        })
        .unwrap();

    assert!(Catalog::open(&ctx, false).unwrap().is_some());
    let options = ContentSearch::default();
    let actual =
        serde_json::to_value(search_content(&ctx, "deletedquartz", &options).unwrap()).unwrap();
    assert!(actual.as_array().unwrap().is_empty());
    let retained = hits(&ctx, "retainedzircon", false);
    assert_eq!(retained.as_array().unwrap().len(), 1);
    let fallback = ContentSearch {
        reindex: true,
        ..options
    };
    assert_eq!(
        actual,
        serde_json::to_value(search_content(&ctx, "deletedquartz", &fallback).unwrap()).unwrap()
    );
    assert_eq!(retained, hits(&ctx, "retainedzircon", true));
}

#[test]
fn genuinely_pruned_change_feed_rejects_generation_and_uses_full_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = corral_core::backend::conformance::fake_fixture();
    let store = Store::init(dir.path())
        .unwrap()
        .with_backend(fixture.backend);
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let task = ctx
        .store
        .create("gap fixture", None, Status::Ready, "obsoletequartz")
        .unwrap();
    drain(&ctx);
    let published = Catalog::open(&ctx, false).unwrap().unwrap();
    let manifest = published.manifest.clone();
    drop(published);
    ctx.store
        .update(&task.id, |task| {
            task.body = "canonicalzircon".into();
            Ok(())
        })
        .unwrap();
    let through = ctx.store.view().unwrap().seq();
    assert!(through.0 > manifest.after);
    (fixture.prune_changes_through)(through).unwrap();
    assert!(matches!(
        ctx.store
            .view()
            .unwrap()
            .changes(corral_core::backend::ChangeSeq(manifest.after), 1),
        Err(corral_core::backend::FeedError::Gap { .. })
    ));
    assert!(super::delta::plan(&ctx, &manifest).unwrap().is_none());
    assert!(Catalog::open(&ctx, false).unwrap().is_none());

    let actual = hits(&ctx, "canonicalzircon", false);
    assert_eq!(actual.as_array().unwrap().len(), 1);
    assert!(
        hits(&ctx, "obsoletequartz", false)
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(actual, hits(&ctx, "canonicalzircon", true));
}

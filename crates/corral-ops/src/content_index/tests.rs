use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::io::Read;

use super::Cache;
use super::VERSION;
use super::read_cache;

thread_local! {
    pub(crate) static SNAPSHOTS: Cell<usize> = const { Cell::new(0) };
    pub(crate) static ARTIFACT_EXTRACTIONS: Cell<usize> = const { Cell::new(0) };
    pub(crate) static CATALOG_EXTRACTIONS: Cell<usize> = const { Cell::new(0) };
    pub(crate) static CATALOG_READS: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn scan_prepared_text_matches_canonical_blob_extraction_without_rereading_sources() {
    use super::{DOCUMENT_BYTES, extract_bytes, index_artifact, index_artifacts};
    let dir = tempfile::tempdir().unwrap();
    let ctx =
        crate::resources::FixedResourceProbe::ctx(corral_core::Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("index fixture", None, corral_core::Status::Ready, "")
        .unwrap();
    let mut split_utf8 = vec![b'x'; DOCUMENT_BYTES - 1];
    split_utf8.extend_from_slice("é".as_bytes());
    for bytes in [
        b"needle\n".to_vec(),
        vec![0; 9000],
        split_utf8,
        vec![b'a'; DOCUMENT_BYTES + 10],
    ] {
        let source = dir.path().join("proof.log");
        std::fs::write(&source, &bytes).unwrap();
        let artifact = crate::add_artifact(&ctx, &task.id, &source, None, None).unwrap();
        let path = ctx.store.cache_root().unwrap().join("content-search.json");
        // Start both cache paths from the same empty state.
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        index_artifact(&ctx, &task.id, &artifact).unwrap();
        let regular = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&source, "source changed after its snapshot").unwrap();
        let prepared = BTreeMap::from([(artifact.sha256.clone(), extract_bytes(&bytes))]);
        let before = ARTIFACT_EXTRACTIONS.get();
        index_artifacts(&ctx, &task.id, &[artifact], &prepared).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), regular);
        assert_eq!(
            ARTIFACT_EXTRACTIONS.get(),
            before,
            "prepared bytes must not reopen blobs"
        );
    }
}

struct CountedFile<'a> {
    file: File,
    reads: &'a Cell<usize>,
}

impl Read for CountedFile<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reads.set(self.reads.get() + 1);
        self.file.read(buffer)
    }
}

#[test]
fn cached_index_is_decoded_with_block_reads_not_per_byte_reads() {
    // Given a real JSON cache file larger than a normal read buffer.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.json");
    let expected = Cache {
        version: VERSION,
        postings: BTreeMap::from([("needle".into(), vec!["x".repeat(32 * 1024)])]),
        ..Cache::default()
    };
    std::fs::write(&path, serde_json::to_vec(&expected).unwrap()).unwrap();
    let reads = Cell::new(0);

    // When the same loader used by search and board sync decodes it.
    let actual = read_cache(CountedFile {
        file: File::open(path).unwrap(),
        reads: &reads,
    });

    // Then bytes still decode identically without one system read per JSON byte.
    assert_eq!(actual.version, expected.version);
    assert_eq!(actual.postings, expected.postings);
    assert!(reads.get() < 16, "{} underlying file reads", reads.get());
}

#[test]
fn warm_search_reads_only_matching_overflow_blobs_at_two_catalog_sizes() {
    use crate::{ContentSearch, search_content};
    use corral_core::work::{self, HotScope, Work};
    use corral_core::{ArtifactKind, PinRef, Status, Store};

    let measure = |unrelated: usize| {
        let dir = tempfile::tempdir().unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
        let task = ctx
            .store
            .create("catalog fixture", None, Status::Ready, "")
            .unwrap();
        let padding = "noise\n".repeat(super::DOCUMENT_BYTES / 6 + 1);
        let source = dir.path().join("hit.log");
        std::fs::write(
            &source,
            format!("{padding}generic_worker_gate_wait_collects_once\n"),
        )
        .unwrap();
        let hit =
            crate::add_artifact(&ctx, &task.id, &source, Some(ArtifactKind::Log), None).unwrap();
        for i in 0..unrelated {
            let source = dir.path().join(format!("unrelated-{i}.log"));
            std::fs::write(&source, format!("{padding}unrelated {i}\n")).unwrap();
            crate::add_artifact(&ctx, &task.id, &source, Some(ArtifactKind::Log), None).unwrap();
        }
        let options = ContentSearch::default();
        let query = "generic_worker_gate_wait_collects_once";
        drain_catalog(&ctx);
        let before = work::snapshot();
        let parses = work::count(Work::EventParse);
        let extractions = CATALOG_EXTRACTIONS.get();
        let reads = CATALOG_READS.get();
        let hits = {
            let _hot = HotScope::enter();
            search_content(&ctx, query, &options).unwrap()
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].reference,
            PinRef::of_artifact(&task.id, &hit).to_string()
        );
        (
            CATALOG_READS.get() - reads,
            CATALOG_EXTRACTIONS.get() - extractions,
            work::snapshot().since(&before).full_task_hydrations(),
            work::count(Work::EventParse) - parses,
        )
    };
    let small = measure(1);
    assert_eq!(small, (1, 0, 0, 0));
    assert_eq!(measure(3), small);
}

#[test]
fn streamed_task_search_preserves_legacy_ranking_after_catalog_changes() {
    use crate::SearchData;
    use corral_core::query::Query;
    use corral_core::{Status, Store};

    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let first = ctx
        .store
        .create("ranking fixture", None, Status::Ready, "body needle")
        .unwrap();
    ctx.store
        .create("other needle", None, Status::Ready, "")
        .unwrap();
    for body in ["initial evidence needle", "changed evidence needle"] {
        ctx.store
            .update(&first.id, |task| {
                task.body = body.into();
                Ok(())
            })
            .unwrap();
        let source = dir.path().join("evidence.log");
        std::fs::write(&source, body).unwrap();
        crate::add_artifact(&ctx, &first.id, &source, None, None).unwrap();
        for input in ["needle", "evidence", "-absent", "status:ready needle"] {
            let query = Query::parse(input, corral_core::clock::now()).unwrap();
            let legacy = SearchData::load(&ctx.store).unwrap();
            let expected: Vec<_> = legacy
                .ranked(&query)
                .into_iter()
                .map(|i| legacy.tasks[i].id.clone())
                .collect();
            assert_eq!(
                SearchData::search_ids(&ctx.store, input, 100).unwrap(),
                expected
            );
        }
    }
}

fn drain_catalog(ctx: &crate::Ctx) {
    for _ in 0..128 {
        if super::catalog::advance_batch(ctx).unwrap() {
            return;
        }
    }
    panic!("tiny catalog fixture did not become ready within 128 batches");
}

#[test]
fn interrupted_catalog_build_resumes_instead_of_restarting() {
    use corral_core::{Status, Store};

    let fixture = || {
        let dir = tempfile::tempdir().unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
        let task = ctx
            .store
            .create("resume fixture", None, Status::Ready, "body needle")
            .unwrap();
        for i in 0..3 {
            let source = dir.path().join(format!("evidence-{i}.log"));
            std::fs::write(&source, format!("artifact needle {i}\n")).unwrap();
            crate::add_artifact(&ctx, &task.id, &source, None, None).unwrap();
        }
        (dir, ctx)
    };
    let (_uninterrupted_dir, uninterrupted) = fixture();
    let before = CATALOG_EXTRACTIONS.get();
    drain_catalog(&uninterrupted);
    let total = CATALOG_EXTRACTIONS.get() - before;
    assert!(total > 1, "fixture must exercise multiple lexical sources");

    let (_interrupted_dir, interrupted) = fixture();
    let before = CATALOG_EXTRACTIONS.get();
    let mut started = false;
    for _ in 0..128 {
        let ready = super::catalog::advance_batch(&interrupted).unwrap();
        if CATALOG_EXTRACTIONS.get() > before {
            assert!(!ready, "source work must checkpoint before publication");
            started = true;
            break;
        }
        assert!(
            !ready,
            "catalog became ready without extracting its sources"
        );
    }
    assert!(started, "no lexical source checkpoint was reached");
    super::catalog::interrupt_after_batches(1);
    assert!(
        super::catalog::advance_batch(&interrupted).is_err(),
        "fault must fire after the next durable batch"
    );
    let checkpointed = CATALOG_EXTRACTIONS.get() - before;
    assert!(checkpointed > 0);
    assert!(
        checkpointed < total,
        "fault must leave source work to resume"
    );

    let before_resume = CATALOG_EXTRACTIONS.get();
    drain_catalog(&interrupted);
    assert_eq!(
        CATALOG_EXTRACTIONS.get() - before_resume,
        total - checkpointed,
        "resuming must extract only sources absent from the durable checkpoint"
    );
}

#[test]
fn query_during_incomplete_catalog_returns_the_same_hits_as_warm() {
    use crate::{ContentSearch, search_content};
    use corral_core::{Status, Store};

    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("parity fixture", None, Status::Ready, "body needle")
        .unwrap();
    let source = dir.path().join("evidence.log");
    std::fs::write(&source, "artifact needle\n").unwrap();
    let artifact = crate::add_artifact(&ctx, &task.id, &source, None, None).unwrap();
    let options = ContentSearch::default();
    let query = || serde_json::to_value(search_content(&ctx, "needle", &options).unwrap()).unwrap();
    let cold = query();
    assert!(
        !super::catalog::advance_batch(&ctx).unwrap(),
        "first batch must leave the fixture catalog incomplete"
    );
    let partial = query();
    drain_catalog(&ctx);
    let warm = query();
    let hits = warm.as_array().unwrap();
    let artifact_ref = corral_core::PinRef::of_artifact(&task.id, &artifact).to_string();
    assert!(hits.iter().any(|hit| hit["reference"] == artifact_ref));
    assert!(hits.iter().any(|hit| {
        hit["snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains("body needle"))
    }));
    assert_eq!(cold, warm);
    assert_eq!(partial, warm);
}

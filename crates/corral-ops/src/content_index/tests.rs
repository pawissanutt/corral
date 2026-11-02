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

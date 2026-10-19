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

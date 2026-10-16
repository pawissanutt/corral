use super::*;

#[test]
fn whole_line_fields_and_excerpts_survive_marker_order_cache_and_compression() {
    // Given both marker orders, at the start and outside every remembered prefix,
    // for cache-resident documents and documents that must be streamed.
    for field in ["error", "test", "verdict"] {
        let mut f = Fixture::new();
        for (prefix, size) in [("cached", 150_000), ("streamed", 350_000)] {
            let padding = "x".repeat(size);
            for (name, text) in [
                (
                    "late",
                    format!("{} needle {padding} {field}\n", "x".repeat(1000)),
                ),
                (
                    "middle",
                    format!("{} {field} {padding} needle\n", "x".repeat(1000)),
                ),
                ("early", format!("{field} {padding} needle\n")),
                ("prefix", format!("needle {padding} {field}\n")),
            ] {
                let name = format!("{prefix}-{name}.log");
                f.add(&name, &text, ArtifactKind::Log, &name);
            }
        }

        // When querying plain, warm gzip, and freshly reindexed gzip snapshots.
        for mode in ["plain", "warm-gzip", "reindexed-gzip"] {
            if mode == "warm-gzip" {
                f.retention(RetentionAction::Compress);
                artifact_gc(&f.ctx, false).unwrap();
                assert!(
                    f.ctx
                        .store
                        .get(&f.task)
                        .unwrap()
                        .artifacts
                        .iter()
                        .all(|a| a.compressed_bytes.is_some())
                );
            }
            let hits = search_content(
                &f.ctx,
                &format!("{field}:needle"),
                &ContentSearch {
                    scope: "artifacts".into(),
                    reindex: mode == "reindexed-gzip",
                    ..Default::default()
                },
            )
            .unwrap();
            let excluded = f.search(&format!("needle -{field}:needle"));

            // Then line-wide membership applies in either direction and each
            // snippet comes from actual matched bytes, not the final chunk.
            assert_eq!(hits.len(), 8, "{field} {mode}: {hits:?}");
            assert!(excluded.is_empty(), "{field} {mode}: {excluded:?}");
            for hit in hits {
                assert_eq!((hit.line_start, hit.line_end), (1, 1));
                assert!(hit.snippet.contains("needle"), "{field} {mode}: {hit:?}");
                assert!(hit.snippet.chars().count() <= 1200);
            }
        }
    }
}

#[test]
fn field_markers_split_at_chunk_boundaries_still_require_whole_words() {
    // Given a true error marker across a boundary and a longer non-marker word
    // whose first chunk ends with a marker spelling.
    let f = Fixture::new();
    for (name, token) in [
        ("marker", "error "),
        ("not-marker", "errorsuffix "),
        ("unicode-suffix", "error\u{e9} "),
    ] {
        let text = format!("needle {} {token}\n", "x".repeat(65_536 - 13));
        f.add(&format!("{name}.log"), &text, ArtifactKind::Log, name);
    }

    // When classifying the source lines rather than individual chunks.
    let hits = f.search("error:needle");

    // Then only the complete marker word qualifies the matching source line.
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "marker");
    assert!(hits[0].snippet.contains("needle"));
}

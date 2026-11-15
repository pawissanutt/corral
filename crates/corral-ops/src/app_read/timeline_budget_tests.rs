use super::*;

#[test]
fn lifecycle_row_cap_retains_complete_pages_with_partial_coverage() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = corral_core::Store::init(tmp.path())?;
    let at: Timestamp = "2026-11-01T00:00:00Z".parse()?;
    let events = (0..(PAGE * 2 + 1))
        .map(|_| {
            serde_json::json!({
                "ts": at, "kind": "commented", "actor": "owner", "detail": {},
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    store.fixture_replace_events(&events)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let mut budget = Budget::new(PAGE as usize + 1);
    let found = history::read(
        &ctx,
        &history::query(None, None, at + jiff::SignedDuration::from_hours(1)),
        &mut budget,
    )?;
    assert_eq!(found.len(), PAGE as usize);
    assert!(budget.coverage.partial);
    assert_eq!(budget.coverage.reason.as_deref(), Some("row_limit"));
    assert_eq!(budget.coverage.scanned_rows, u64::from(PAGE));
    assert_eq!(budget.coverage.scanned_from, Some(at.to_string()));
    assert_eq!(budget.coverage.scanned_to, Some(at.to_string()));
    Ok(())
}

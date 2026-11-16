use super::*;
use corral_core::{Status, Store};
use corral_protocol::detail::CLOSED_ANCHOR_MAX;

#[test]
fn interleaved_closed_statuses_page_without_gaps_or_duplicate_timestamp_ties() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let at: jiff::Timestamp = "2026-01-01T00:00:00Z".parse()?;
    let mut ids = Vec::new();
    for (status, hours) in [
        (Status::Done, 3),
        (Status::Dropped, 4),
        (Status::Done, 4),
        (Status::Dropped, 2),
        (Status::Done, 1),
        (Status::Backlog, 5),
    ] {
        let mut task = store.create("closed fixture", Some("project"), status, "")?;
        task.updated = at + jiff::SignedDuration::from_hours(hours);
        store.fixture_put_task(&task)?;
        ids.push(task.id);
    }
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let expected = [&ids[2], &ids[1], &ids[0], &ids[3], &ids[4]];
    let mut before = None;
    let mut found = Vec::new();
    for size in [2, 2, 1] {
        let page = ctx.read_step(|ctx| closed::page_with_limit(ctx, before.as_deref(), 2))?;
        assert_eq!(page.total, Some(5));
        assert_eq!(page.rows.len(), size);
        for row in page.rows {
            assert!(matches!(row.status.as_str(), "done" | "dropped"));
            assert_eq!(row.project.as_deref(), Some("project"));
            assert!(row.updated.parse::<jiff::Timestamp>().is_ok());
            found.push(row.id);
        }
        before = page.older;
        assert_eq!(before.is_some(), found.len() < expected.len());
        if let Some(anchor) = &before {
            assert!(anchor.len() <= CLOSED_ANCHOR_MAX);
        }
    }
    assert_eq!(found.iter().collect::<Vec<_>>(), expected);

    let request = DetailGet {
        target: DetailRef::Closed { before: None },
        etag: None,
        page: None,
    };
    let reply = read(&ctx, &request)?;
    assert!(reply.next.is_none());
    let DetailBody::Closed(page) = reply.body.unwrap() else {
        panic!("closed selector returned another detail type");
    };
    assert_eq!(
        page.rows.iter().map(|row| &row.id).collect::<Vec<_>>(),
        expected
    );
    assert!(page.older.is_none());
    assert!(
        read(
            &ctx,
            &DetailGet {
                etag: Some(reply.etag),
                ..request
            }
        )?
        .body
        .is_none()
    );

    for before in [
        String::new(),
        "not-an-anchor".into(),
        "x".repeat(CLOSED_ANCHOR_MAX + 1),
    ] {
        let error = read(
            &ctx,
            &DetailGet {
                target: DetailRef::Closed {
                    before: Some(before),
                },
                etag: None,
                page: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid closed before anchor");
    }
    Ok(())
}

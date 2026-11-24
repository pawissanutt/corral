use corral_types::SystemDataRow;

use super::*;

#[test]
fn socket_system_data_pages_are_bound_to_the_selected_commit() {
    let fixture = Fixture::with_repository();
    let repo = fixture.root().join("repo");
    let schema = (0..101)
        .map(|n| {
            let properties = if n == 0 {
                json!({"member": {"$ref": "#/$defs/Record1"}})
            } else {
                json!({"id": {"type": "integer"}})
            };
            (
                format!("Record{n}"),
                json!({"type": "object", "properties": properties}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    fs::write(
        repo.join("model.schema.json"),
        json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "$defs": schema})
            .to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("client.py"),
        "# No built-in data reader for Python.\n",
    )
    .unwrap();
    fs::write(
        repo.join("broken.rs"),
        "#[derive(Serialize)] struct Broken {",
    )
    .unwrap();
    git(
        &repo,
        &["add", "model.schema.json", "client.py", "broken.rs"],
    );
    git(&repo, &["commit", "-m", "add data schema"]);
    let historical = corral_facts::resolve_revision(&repo, "HEAD").unwrap();
    fs::remove_file(repo.join("model.schema.json")).unwrap();
    git(&repo, &["commit", "-am", "remove data schema"]);

    let _poll = PollGuard::start(fixture.shared());
    let path = fixture.unix_server();
    let (mut legacy, _) = connect(&path, Vec::new());
    let (mut client, _) = connect(&path, vec![SYSTEM_V1.into()]);
    let query = DetailGet {
        target: DetailRef::SystemData {
            project: "demo".into(),
            at: Some(historical.commit().to_owned()),
        },
        etag: None,
        page: None,
    };
    assert_eq!(
        legacy.call(DETAIL_GET, &query).unwrap_err().code,
        rpc::ErrorCode::InvalidParams
    );
    let deadline = Instant::now() + WAIT;
    let (first, mut rows) = loop {
        let mut reply = client.ok::<DetailReply>(DETAIL_GET, &query);
        let Some(DetailBody::SystemData(body)) = reply.body.take() else {
            panic!("expected a system data detail");
        };
        if body.status == SystemStatus::Ready {
            assert_eq!(body.error, None);
            assert_eq!(body.commit.as_deref(), Some(historical.commit()));
            break (reply, body.rows);
        }
        assert!(
            Instant::now() < deadline,
            "waiting for historical data snapshot"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let next = first.next.expect("historical data must be paged");
    let wrong = DetailGet {
        target: DetailRef::SystemData {
            project: "demo".into(),
            at: None,
        },
        page: Some(next.clone()),
        etag: None,
    };
    let error = client.call(DETAIL_GET, wrong).unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    let last = client.ok::<DetailReply>(
        DETAIL_GET,
        DetailGet {
            page: Some(next),
            ..query
        },
    );
    assert_eq!(last.etag, first.etag);
    assert!(last.next.is_none());
    let Some(DetailBody::SystemData(last)) = last.body else {
        panic!("expected a second system data page");
    };
    assert_eq!(last.commit.as_deref(), Some(historical.commit()));
    rows.extend(last.rows);
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, SystemDataRow::Type { .. }))
            .count(),
        101
    );
    assert!(rows.iter().any(|row| matches!(row,
        SystemDataRow::Coverage { language, sources } if language == "Python" && sources.is_empty()
    )));
    assert!(rows.iter().any(|row| matches!(row,
        SystemDataRow::Has { edge } if edge.from.name == "Record0" && edge.to.name == "Record1"
    )));
    assert!(rows.iter().any(|row| matches!(row,
        SystemDataRow::Unreadable { path } if path == "broken.rs"
    )));
}

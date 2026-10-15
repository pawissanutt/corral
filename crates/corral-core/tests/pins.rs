use corral_core::*;
use jiff::Timestamp;
use tempfile::tempdir;

fn add_artifact(s: &Store, id: &str, run: &str, sha8: &str, file: &str, title: &str, at: i64) {
    let rel = format!("artifacts/{id}/{run}/{sha8}-{file}");
    let abs = s.dir().join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, format!("{sha8}\n")).unwrap();
    s.update(id, |t| {
        t.artifacts.push(Artifact {
            run: run.into(),
            kind: ArtifactKind::Report,
            title: title.into(),
            path: rel.clone(),
            source: format!("/src/{file}"),
            sha256: sha8.repeat(8),
            bytes: 9,
            added: Timestamp::from_second(at).unwrap(),
        });
        Ok(())
    })
    .unwrap();
}

fn refs(s: &Store) -> Vec<String> {
    s.pins().unwrap().into_iter().map(|p| p.target).collect()
}

#[test]
fn pin_unpin_round_trip_and_order() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    s.create("A", None, Status::Backlog, "").unwrap();
    s.create("B", None, Status::Backlog, "").unwrap();
    add_artifact(&s, "T-001", "R1", "aaaaaaaa", "report.html", "Report", 10);
    assert!(s.pins().unwrap().is_empty());

    assert_eq!(s.pin("2", Some("bee"), false).unwrap().0, 1);
    let (n, p) = s.pin("t-1:r1:report.html", None, true).unwrap();
    assert_eq!(n, 2);
    assert_eq!(p.target, "T-001:R1:aaaaaaaa-report.html");
    assert_eq!(p.follow, Some(Follow::Latest));
    s.pin("T-001", None, false).unwrap();
    assert_eq!(
        refs(&s),
        ["T-002", "T-001:R1:aaaaaaaa-report.html", "T-001"]
    );

    let (n, p) = s.pin("T-002", Some("renamed"), false).unwrap();
    assert_eq!(n, 1, "re-pin keeps its position");
    assert_eq!(p.label.as_deref(), Some("renamed"));
    assert_eq!(s.pins().unwrap().len(), 3);

    let text = std::fs::read_to_string(s.dir().join("pins.toml")).unwrap();
    assert!(
        text.contains("[[pin]]") && text.contains("follow = \"latest\""),
        "{text}"
    );

    assert!(
        s.pin("T-001", None, true).is_err(),
        "follow needs an artifact"
    );
    assert!(s.pin("T-009", None, false).is_err());
    assert!(s.pin("T-001:R1:nope.html", None, false).is_err());

    assert_eq!(
        s.unpin("T-001:R1:report.html").unwrap().target,
        "T-001:R1:aaaaaaaa-report.html"
    );
    assert_eq!(refs(&s), ["T-002", "T-001"]);
    assert_eq!(s.unpin("2").unwrap().target, "T-001");
    assert!(s.unpin("5").is_err());
    assert!(s.unpin("T-001").is_err());
    assert_eq!(refs(&s), ["T-002"]);
    let kinds: Vec<String> = s.events().unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"pin".to_string()) && kinds.contains(&"unpin".to_string()));
}

#[test]
fn move_and_label_pins() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    for t in ["A", "B", "C"] {
        s.create(t, None, Status::Backlog, "").unwrap();
    }
    for id in ["T-001", "T-002", "T-003"] {
        s.pin(id, None, false).unwrap();
    }

    assert_eq!(s.move_pin(2, 0).unwrap(), 0);
    assert_eq!(refs(&s), ["T-003", "T-001", "T-002"]);
    assert_eq!(s.move_pin(0, 9).unwrap(), 2, "target clamps to the end");
    assert_eq!(refs(&s), ["T-001", "T-002", "T-003"]);
    assert!(s.move_pin(3, 0).is_err());

    assert_eq!(
        s.label_pin(1, Some(" bee ")).unwrap().label.as_deref(),
        Some("bee")
    );
    assert_eq!(s.pins().unwrap()[1].label.as_deref(), Some("bee"));
    assert_eq!(s.label_pin(1, Some("  ")).unwrap().label, None);
    assert!(s.label_pin(5, Some("x")).is_err());
    let kinds: Vec<String> = s.events().unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"pin_move".to_string()), "{kinds:?}");
}

#[test]
fn follow_latest_resolves_newest_matching() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    s.create("A", None, Status::Backlog, "").unwrap();
    add_artifact(&s, "T-001", "R1", "aaaaaaaa", "dash.html", "Dashboard", 10);
    add_artifact(&s, "T-001", "R1", "bbbbbbbb", "other.md", "Other", 20);
    s.pin("T-001:R1:dash.html", None, true).unwrap();
    s.pin("T-001:R1:other.md", None, false).unwrap();
    s.pin("T-001", Some("task"), false).unwrap();

    add_artifact(&s, "T-001", "R2", "cccccccc", "dash.html", "Dashboard", 30);
    add_artifact(
        &s,
        "T-001",
        "R3",
        "dddddddd",
        "renamed.html",
        "Dashboard",
        40,
    );
    add_artifact(&s, "T-001", "R3", "eeeeeeee", "late.md", "Other", 50);

    let resolved = s.resolved_pins().unwrap();
    let PinTarget::Artifact {
        artifact,
        followed,
        path,
        ..
    } = &resolved[0].1
    else {
        panic!("{:?}", resolved[0].1)
    };
    assert_eq!(artifact.path, "artifacts/T-001/R3/dddddddd-renamed.html");
    assert!(*followed);
    assert!(path.is_file());
    let PinTarget::Artifact {
        artifact, followed, ..
    } = &resolved[1].1
    else {
        panic!("{:?}", resolved[1].1)
    };
    assert_eq!(artifact.path, "artifacts/T-001/R1/bbbbbbbb-other.md");
    assert!(!followed, "a pin without follow stays on its snapshot");
    assert!(matches!(&resolved[2].1, PinTarget::Task { id, .. } if id == "T-001"));
}

#[test]
fn broken_pins_are_reported_not_dropped() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    s.create("A", None, Status::Backlog, "").unwrap();
    let b = s.create("B", None, Status::Backlog, "").unwrap();
    add_artifact(&s, "T-001", "R1", "aaaaaaaa", "shot.png", "Shot", 10);
    s.pin("T-001:R1:shot.png", None, false).unwrap();
    s.pin("T-002", None, false).unwrap();
    s.pin("T-001", None, false).unwrap();

    std::fs::remove_file(s.dir().join("artifacts/T-001/R1/aaaaaaaa-shot.png")).unwrap();
    std::fs::remove_file(&b.path).unwrap();

    let resolved = s.resolved_pins().unwrap();
    assert_eq!(resolved.len(), 3);
    assert!(
        matches!(&resolved[0].1, PinTarget::Broken(r) if r.contains("missing")),
        "{:?}",
        resolved[0].1
    );
    assert!(
        matches!(&resolved[1].1, PinTarget::Broken(r) if r.contains("T-002 not found")),
        "{:?}",
        resolved[1].1
    );
    assert!(matches!(&resolved[2].1, PinTarget::Task { .. }));
    assert_eq!(s.pins().unwrap().len(), 3);
}

use super::*;
fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    (dir, store)
}
fn add(kind: NoteKind, text: &str) -> NoteMutation {
    NoteMutation::Add {
        kind,
        text: text.into(),
        provenance: None,
        reason: None,
        resume_when: None,
    }
}
fn supersede(id: &str) -> NoteMutation {
    NoteMutation::Supersede {
        id: id.into(),
        text: "new".into(),
        provenance: None,
        reason: None,
        resume_when: None,
    }
}
#[test]
fn missing_area_is_empty_revision_zero() {
    let (_dir, store) = fixture();
    let notes = load(&store, "workspace").unwrap();
    assert_eq!(notes.revision, 0);
    assert!(notes.entries.is_empty());
}
#[test]
fn stale_revision_is_typed_conflict() {
    let (_dir, store) = fixture();
    let error = apply(
        &store,
        "workspace",
        Some(1),
        NoteAuthor::Owner,
        add(NoteKind::Hypothesis, "x"),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<NotesError>(),
        Some(&NotesError::Conflict {
            expected: 1,
            actual: 0
        })
    );
}
#[test]
fn pm_instruction_requires_provenance() {
    let (_dir, store) = fixture();
    assert!(
        apply(
            &store,
            "workspace",
            None,
            NoteAuthor::Pm,
            add(NoteKind::Instruction, "x")
        )
        .is_err()
    );
    assert_eq!(load(&store, "workspace").unwrap().revision, 0);
}
#[test]
fn pm_cannot_supersede_or_remove_owner_entry() {
    let (_dir, store) = fixture();
    apply(
        &store,
        "workspace",
        None,
        NoteAuthor::Owner,
        add(NoteKind::Hypothesis, "x"),
    )
    .unwrap();
    for mutation in [supersede("n-1"), NoteMutation::Remove { id: "n-1".into() }] {
        let error = apply(&store, "workspace", None, NoteAuthor::Pm, mutation).unwrap_err();
        assert_eq!(
            error.downcast_ref::<NotesError>(),
            Some(&NotesError::OwnerEntry)
        );
    }
}
#[test]
fn supersession_links_both_entries_and_preserves_kind() {
    let (_dir, store) = fixture();
    apply(
        &store,
        "workspace",
        None,
        NoteAuthor::Pm,
        add(NoteKind::Commitment, "x"),
    )
    .unwrap();
    let notes = apply(
        &store,
        "workspace",
        Some(1),
        NoteAuthor::Pm,
        supersede("n-1"),
    )
    .unwrap();
    assert_eq!(notes.revision, 2);
    assert_eq!(notes.entries[0].superseded_by.as_deref(), Some("n-2"));
    assert_eq!(notes.entries[1].supersedes.as_deref(), Some("n-1"));
    assert_eq!(notes.entries[1].kind, NoteKind::Commitment);
}
#[test]
fn bounds_refuse_oversize_text_fields_and_live_entries() {
    let (_dir, store) = fixture();
    assert!(
        apply(
            &store,
            "workspace",
            None,
            NoteAuthor::Owner,
            add(NoteKind::Hypothesis, &"é".repeat(513))
        )
        .is_err()
    );
    let mut mutation = add(NoteKind::Hypothesis, "x");
    if let NoteMutation::Add { provenance, .. } = &mut mutation {
        *provenance = Some("x".repeat(257));
    }
    assert!(apply(&store, "workspace", None, NoteAuthor::Owner, mutation).is_err());
    for _ in 0..MAX_LIVE {
        apply(
            &store,
            "workspace",
            None,
            NoteAuthor::Owner,
            add(NoteKind::Hypothesis, "x"),
        )
        .unwrap();
    }
    assert!(
        apply(
            &store,
            "workspace",
            None,
            NoteAuthor::Owner,
            add(NoteKind::Hypothesis, "x")
        )
        .is_err()
    );
    assert_eq!(load(&store, "workspace").unwrap().entries.len(), MAX_LIVE);
}
#[test]
fn record_byte_bound_refuses_without_writing() {
    let (_dir, store) = fixture();
    let mut successful = 0;
    while apply(
        &store,
        "workspace",
        None,
        NoteAuthor::Owner,
        add(NoteKind::Hypothesis, &"x".repeat(MAX_TEXT_BYTES)),
    )
    .is_ok()
    {
        successful += 1;
    }
    assert!(successful < MAX_LIVE);
    assert_eq!(
        load(&store, "workspace").unwrap().revision,
        successful as u64
    );
}
#[test]
fn superseded_retention_drops_oldest_and_deferrals_require_conditions() {
    let (_dir, store) = fixture();
    assert!(
        apply(
            &store,
            "workspace",
            None,
            NoteAuthor::Owner,
            add(NoteKind::Deferral, "x")
        )
        .is_err()
    );
    apply(
        &store,
        "workspace",
        None,
        NoteAuthor::Owner,
        add(NoteKind::Hypothesis, "x"),
    )
    .unwrap();
    for id in 1..=MAX_SUPERSEDED + 1 {
        apply(
            &store,
            "workspace",
            None,
            NoteAuthor::Owner,
            supersede(&format!("n-{id}")),
        )
        .unwrap();
    }
    let notes = load(&store, "workspace").unwrap();
    assert_eq!(notes.entries.len(), MAX_SUPERSEDED + 1);
    assert_eq!(notes.entries[0].id, "n-2");
}

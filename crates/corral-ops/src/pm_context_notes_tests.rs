use super::*;
use corral_core::agent::pm_notes::NoteMutation;

fn add(store: &Store, author: NoteAuthor, kind: NoteKind, text: &str) -> PmNotes {
    pm_notes::apply(
        store,
        "workspace",
        None,
        author,
        NoteMutation::Add {
            kind,
            text: text.into(),
            provenance: Some("owner-message:1".into()),
            reason: (kind == NoteKind::Deferral).then(|| "waiting for review".into()),
            resume_when: (kind == NoteKind::Deferral).then(|| "review answered".into()),
        },
    )
    .unwrap()
}

fn read_snapshot(ctx: &crate::Ctx) -> (Vec<u8>, Value) {
    let bytes = snapshot(
        &ctx.store,
        json!({
            "version": 1,
            "workspace": ctx.store.root(),
            "projects": {},
            "policy": {},
        }),
    )
    .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    (bytes, value)
}

#[test]
fn launch_snapshot_freezes_the_validated_cache_key_setting() {
    let root = tempfile::tempdir().unwrap();
    let ctx = crate::Ctx::from_store(Store::init(root.path()).unwrap());
    let (_, context) = read_snapshot(&ctx);
    assert_eq!(context["cache_key"], json!({"mode": "session"}));
    let layers = ctx.store.config_layers().unwrap();
    let path = layers.path(corral_core::ConfigLayer::Workspace).unwrap();
    std::fs::write(
        path,
        "[pm]\nagent = \"omo\"\n[pm.cache_key]\nmode = \"stable\"\n",
    )
    .unwrap();
    let (_, stable) = read_snapshot(&ctx);
    assert_eq!(stable["cache_key"], json!({"mode": "stable"}));
    assert_eq!(context["cache_key"], json!({"mode": "session"}));
}

#[test]
fn launch_notes_include_only_live_entries_in_priority_order_at_current_revision() {
    let root = tempfile::tempdir().unwrap();
    let ctx = crate::Ctx::from_store(Store::init(root.path()).unwrap());
    let initial = add(
        &ctx.store,
        NoteAuthor::Pm,
        NoteKind::Deferral,
        "old deferral",
    );
    let old_id = initial.entries[0].id.clone();
    let instruction = add(
        &ctx.store,
        NoteAuthor::Owner,
        NoteKind::Instruction,
        "owner instruction",
    );
    let owner_id = instruction.entries[1].id.clone();
    let current = pm_notes::apply(
        &ctx.store,
        "workspace",
        None,
        NoteAuthor::Pm,
        NoteMutation::Supersede {
            id: old_id.clone(),
            text: "current deferral".into(),
            provenance: None,
            reason: Some("waiting for review".into()),
            resume_when: Some("review answered".into()),
        },
    )
    .unwrap();
    let (_, snapshot) = read_snapshot(&ctx);
    let notes = &snapshot["notes"];
    assert_eq!(notes["revision"], current.revision);
    assert_eq!(notes["omitted"], 0);
    let entries = notes["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["id"], owner_id);
    assert_eq!(entries[0]["kind"], "instruction");
    assert_eq!(entries[0]["provenance"], "owner-message:1");
    assert_eq!(entries[1]["kind"], "deferral");
    assert_eq!(entries[1]["resume_when"], "review answered");
    assert!(entries.iter().all(|entry| entry["id"] != old_id));
    assert!(
        entries
            .iter()
            .all(|entry| entry.get("superseded_by").is_none())
    );
}

#[test]
fn oversized_launch_notes_drop_the_lowest_priority_tail_within_both_caps() {
    let root = tempfile::tempdir().unwrap();
    let ctx = crate::Ctx::from_store(Store::init(root.path()).unwrap());
    let instruction = add(
        &ctx.store,
        NoteAuthor::Owner,
        NoteKind::Instruction,
        "keep first",
    );
    for _ in 0..24 {
        add(
            &ctx.store,
            NoteAuthor::Pm,
            NoteKind::Hypothesis,
            &"x".repeat(1024),
        );
    }
    let (bytes, snapshot) = read_snapshot(&ctx);
    let notes = &snapshot["notes"];
    let entries = notes["entries"].as_array().unwrap();
    let omitted = notes["omitted"].as_u64().unwrap() as usize;
    assert!(omitted > 0);
    assert_eq!(entries.len() + omitted, 25);
    assert_eq!(entries[0]["id"], instruction.entries[0].id);
    assert!(serde_json::to_vec(notes).unwrap().len() <= NOTES_MAX_BYTES);
    assert!(bytes.len() <= SNAPSHOT_MAX_BYTES);
    assert_eq!(notes["revision"], 25);
}

#[test]
fn notes_order_owner_instructions_before_pm_then_deferrals_commitments_and_hypotheses() {
    let entries = [
        ("hypothesis", "pm", 1),
        ("commitment", "pm", 1),
        ("deferral", "pm", 1),
        ("instruction", "pm", 3),
        ("instruction", "owner", 1),
        ("instruction", "owner", 2),
        ("deferral", "pm", 2),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, author, day))| {
        serde_json::from_value(json!({
            "id": format!("n-{index}"),
            "kind": kind,
            "author": author,
            "text": "context",
            "created_at": format!("2026-11-{day:02}T00:00:00Z"),
        }))
        .unwrap()
    })
    .collect();
    let notes = PmNotes {
        entries,
        ..Default::default()
    };
    let block = project(&notes, NOTES_MAX_BYTES).unwrap();
    let ids: Vec<_> = block["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["n-5", "n-4", "n-3", "n-6", "n-2", "n-1", "n-0"]);
}

use super::*;

#[test]
fn pinned_history_cursor_survives_a_journal_append() -> Result<()> {
    let first = json!({"type":"message", "id":"first", "parentId":null,"message":{"role":"user","content":"history"}});
    let latest = json!({"type":"message", "id":"last", "parentId":"first","message":{"role":"assistant","content":(0..12).map(|_| json!({"type":"text","text":"x".repeat(3000)})).collect::<Vec<_>>()}});
    let (_dir, path) = fixture([first, latest], "")?;
    let tail = page(ID, &path, None, None)?;
    let cursor = tail.older.as_deref().context("expected a split cursor")?;
    let earlier = page(ID, &path, None, Some(cursor))?;
    let mut file = fs::OpenOptions::new().append(true).open(&path)?;
    writeln!(
        file,
        "{}",
        json!({"type":"message","id":"next","parentId":"last","message":{"role":"user","content":"new words"}})
    )?;
    let still_earlier = page(ID, &path, None, Some(cursor))?;
    assert_eq!(earlier.messages, still_earlier.messages);
    assert_eq!(earlier.older, still_earlier.older);
    assert_ne!(earlier.stamp, still_earlier.stamp);
    Ok(())
}

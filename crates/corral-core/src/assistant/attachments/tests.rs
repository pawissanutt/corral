use super::super::threads::Message;
use super::*;

fn declared(bytes: &[u8]) -> Upload {
    Upload {
        kind: "file".into(),
        name: "notes.txt".into(),
        media_type: "text/plain".into(),
        bytes: bytes.len() as u64,
        sha256: BlobHash::of(bytes).to_string(),
        received: 0,
        created: String::new(),
        blob: None,
    }
}
fn ready(store: &Store, id: &str, bytes: &[u8]) -> Upload {
    store
        .app_upload_begin("device", id, declared(bytes))
        .unwrap();
    store
        .app_upload_put("device", id, 0, &STANDARD.encode(bytes))
        .unwrap()
        .0
}
fn message(store: &Store, device: &str, cmd: &str) -> Message {
    let thread = store.app_thread("quick").unwrap();
    Message {
        id: String::new(),
        session: thread.session,
        seq: 0,
        cmd_id: cmd.into(),
        device: device.into(),
        name: "owner".into(),
        target: "assistant".into(),
        text: "typed words".into(),
        typed_at: store.now().to_string(),
        at: store.now().to_string(),
        state: "sent".into(),
        input: None,
        pm_item: None,
        saved_task: None,
        reply: None,
        failure: None,
        turn: None,
        generation: None,
        late: false,
        tainted: false,
        model: None,
        context: Vec::new(),
        rev: 1,
        attachments: Vec::new(),
    }
}

#[test]
fn chunk_replay_keeps_progress_and_checks_final_digest() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    // Smallest file that crosses the chunk boundary.
    let bytes = vec![b'x'; ATTACHMENT_CHUNK + 1];
    let new = declared(&bytes);
    store.app_upload_begin("device", "u", new.clone()).unwrap();
    let first = STANDARD.encode(&bytes[..ATTACHMENT_CHUNK]);
    assert_eq!(
        store
            .app_upload_put("device", "u", 0, &first)
            .unwrap()
            .0
            .received,
        ATTACHMENT_CHUNK as u64
    );
    assert!(store.app_upload_put("device", "u", 0, &first).unwrap().1);
    assert!(
        store
            .app_upload_put(
                "device",
                "u",
                0,
                &STANDARD.encode(vec![b'y'; ATTACHMENT_CHUNK])
            )
            .is_err()
    );
    let resumed = store.app_upload_begin("device", "u", new.clone()).unwrap();
    assert!(resumed.1);
    assert_eq!(resumed.0.received, ATTACHMENT_CHUNK as u64);
    let mut changed = new;
    changed.name = "other.txt".into();
    assert!(
        store
            .app_upload_begin("device", "u", changed)
            .unwrap_err()
            .to_string()
            .contains("payload_mismatch")
    );
    let final_chunk = STANDARD.encode(&bytes[ATTACHMENT_CHUNK..]);
    let done = store
        .app_upload_put("device", "u", ATTACHMENT_CHUNK as u64, &final_chunk)
        .unwrap()
        .0;
    assert!(done.blob.is_some());
    assert!(
        store
            .app_upload_put("device", "u", ATTACHMENT_CHUNK as u64, &final_chunk)
            .unwrap()
            .1
    );
    assert!(store.app_upload_put("device", "u", 0, &first).unwrap().1);
    assert!(
        store
            .record::<Part>(PARTS, &part_key("device", "u", 0))
            .unwrap()
            .is_none()
    );
    let mut bad = declared(b"yes");
    bad.sha256 = BlobHash::of(b"no!").to_string();
    store.app_upload_begin("device", "bad", bad).unwrap();
    assert!(
        store
            .app_upload_put("device", "bad", 0, &STANDARD.encode(b"yes"))
            .unwrap_err()
            .to_string()
            .contains("digest_mismatch")
    );
    assert!(
        !store
            .record::<DeviceUploads>(UPLOADS, "device")
            .unwrap()
            .unwrap()
            .uploads
            .contains_key("bad")
    );
}

#[test]
fn foreign_upload_and_failed_append_leave_the_reference_ready() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let item = ready(&store, "u", b"hello");
    let refs = vec![("u".into(), item.sha256.clone())];
    let foreign = message(&store, "foreign", "send");
    assert!(
        store
            .append_app_message(foreign, "foreign-digest", &refs)
            .is_err()
    );
    let local = message(&store, "device", "send");
    for invalid in ["absent", "u"] {
        let mut attempted = refs.clone();
        // Neither a missing nor a duplicate reference may consume the valid first upload.
        attempted.push((invalid.into(), item.sha256.clone()));
        assert!(
            store
                .append_app_message(local.clone(), "digest", &attempted)
                .is_err()
        );
    }
    assert!(store.app_send_entry("device", "send").unwrap().is_none());
    assert!(
        store
            .record::<DeviceUploads>(UPLOADS, "device")
            .unwrap()
            .unwrap()
            .uploads
            .contains_key("u")
    );
    let (sent, replayed) = store
        .append_app_message(local.clone(), "digest", &refs)
        .unwrap();
    assert!(!replayed);
    assert_eq!(sent.attachments.len(), 1);
    assert_eq!(sent.attachments[0].id, "a1");
    assert!(
        store
            .record::<DeviceUploads>(UPLOADS, "device")
            .unwrap()
            .is_none()
    );
    let blob = item.blob.unwrap();
    let row = store.blob_row(blob.hash()).unwrap().unwrap();
    assert_eq!(row.refs, 1);
    assert!(
        row.unreferenced_since.is_none(),
        "reference moved atomically, never becoming unreferenced"
    );
    assert!(
        store
            .append_app_message(local.clone(), "digest", &refs)
            .unwrap()
            .1,
        "canonical retry precedes consumed upload lookup"
    );
    assert!(
        store
            .append_app_message(message(&store, "device", "second"), "second", &refs)
            .is_err()
    );
    assert!(store.append_app_message(local, "changed", &refs).is_err());
    assert!(
        store
            .update_app_message(&sent.id, |message| {
                message.attachments.clear();
                Ok(())
            })
            .is_err()
    );
    let cancelled = store.cancel_app_send("device", "send").unwrap().unwrap();
    assert!(cancelled.0);
    assert_eq!(cancelled.1.attachments, sent.attachments);
    assert_eq!(store.blob_row(blob.hash()).unwrap().unwrap().refs, 1);
    assert!(store.cancel_app_send("device", "send").unwrap().unwrap().0);
    assert!(store.cancel_app_send("device", "unseen").unwrap().is_none());
    assert!(store.app_send_entry("device", "unseen").unwrap().is_none());
    let (delivered, _) = store
        .append_app_message(message(&store, "device", "delivered"), "delivered", &[])
        .unwrap();
    store
        .update_app_message(&delivered.id, |message| {
            message.state = "delivered".into();
            Ok(())
        })
        .unwrap();
    let result = store
        .cancel_app_send("device", "delivered")
        .unwrap()
        .unwrap();
    assert!(!result.0);
    assert_eq!(result.1.state, "delivered");
    assert!(
        store
            .cancel_app_send("foreign", "delivered")
            .unwrap()
            .is_none()
    );
}

#[test]
fn legacy_quota_fields_do_not_limit_uploads_or_prevent_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let expired = ready(&store, "old", b"expired");
    let live = ready(&store, "live", b"revoked");
    let mut state = store
        .record::<serde_json::Value>(UPLOADS, "device")
        .unwrap()
        .unwrap();
    state["uploads"]["old"]["created"] = serde_json::json!("2000-01-01T00:00:00Z");
    state["day"] = serde_json::json!(store.now().as_second().div_euclid(TTL).to_string());
    state["day_bytes"] = serde_json::json!(64 * 1024 * 1024);
    store.save_record(UPLOADS, "device", &state).unwrap();
    store
        .save_record(
            UPLOADS,
            "legacy-empty",
            &serde_json::json!({"uploads": {}, "day": state["day"], "day_bytes": state["day_bytes"]}),
        )
        .unwrap();

    // Reserve metadata only: 33 individually valid images cross both former quotas.
    let image = Upload {
        kind: "image".into(),
        media_type: "image/png".into(),
        bytes: IMAGE_MAX,
        ..declared(b"not uploaded")
    };
    for n in 0..33 {
        store
            .app_upload_begin("device", &format!("image-{n}"), image.clone())
            .unwrap();
    }
    let state = store
        .record::<DeviceUploads>(UPLOADS, "device")
        .unwrap()
        .unwrap();
    assert_eq!(state.uploads.len(), 34);
    assert_eq!(state.uploads["live"], live);
    assert_eq!(
        store
            .blob_row(expired.blob.unwrap().hash())
            .unwrap()
            .unwrap()
            .refs,
        0
    );

    // Revocation removes both pending parts and ready references.
    let first = STANDARD.encode(vec![0; ATTACHMENT_CHUNK]);
    store
        .app_upload_put("device", "image-0", 0, &first)
        .unwrap();
    store.app_upload_sweep(|id| id != "device").unwrap();
    for device in ["device", "legacy-empty"] {
        assert!(
            store
                .record::<DeviceUploads>(UPLOADS, device)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        store
            .record::<Part>(PARTS, &part_key("device", "image-0", 0))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .blob_row(live.blob.unwrap().hash())
            .unwrap()
            .unwrap()
            .refs,
        0
    );

    let discarded = ready(&store, "discarded", b"unclaimed");
    store.app_upload_discard("device", "discarded").unwrap();
    assert!(
        store
            .record::<DeviceUploads>(UPLOADS, "device")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .blob_row(discarded.blob.unwrap().hash())
            .unwrap()
            .unwrap()
            .refs,
        0
    );
}

#[test]
fn message_keeps_order_and_blob_ownership_beyond_four_attachments() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let mut outgoing = message(&store, "device", "many");
    let mut refs = Vec::new();
    for n in 0..10 {
        let id = format!("upload-{n}");
        let upload = ready(&store, &id, id.as_bytes());
        refs.push((id.clone(), upload.sha256));
        outgoing.attachments.push(Attachment {
            id,
            kind: "upload".into(),
            name: String::new(),
            media_type: None,
            bytes: 0,
            sha256: None,
            blob: None,
            url: None,
        });
    }
    // Placeholder order rather than claim order determines display IDs.
    refs.reverse();
    let (sent, _) = store.append_app_message(outgoing, "digest", &refs).unwrap();
    let updated = store
        .update_app_message(&sent.id, |msg| {
            msg.state = "done".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(updated.attachments, sent.attachments);
    assert_eq!(updated.attachments.len(), refs.len());
    store.app_upload_sweep(|_| false).unwrap();
    for (n, (attachment, (_, digest))) in updated
        .attachments
        .iter()
        .zip(refs.iter().rev())
        .enumerate()
    {
        assert_eq!(attachment.id, format!("a{}", n + 1));
        assert_eq!(attachment.sha256.as_ref(), Some(digest));
        let row = store
            .blob_row(attachment.blob.as_ref().unwrap().hash())
            .unwrap()
            .unwrap();
        assert_eq!(row.refs, 1);
        assert!(row.unreferenced_since.is_none());
    }
}

#[test]
fn per_file_size_chunk_and_content_checks_still_apply() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let oversized = Upload {
        bytes: TEXT_MAX + 1,
        ..declared(b"x")
    };
    assert!(
        store
            .app_upload_begin("device", "oversized", oversized)
            .is_err()
    );
    store
        .app_upload_begin("device", "binary", declared(b"a\0b"))
        .unwrap();
    assert!(
        store
            .app_upload_put("device", "binary", 0, &STANDARD.encode(b"a\0b"))
            .unwrap_err()
            .to_string()
            .contains("attachment_type_unsupported")
    );
    store
        .app_upload_begin("device", "small", declared(b"x"))
        .unwrap();
    assert!(
        store
            .app_upload_put(
                "device",
                "small",
                0,
                &"A".repeat(ATTACHMENT_CHUNK.div_ceil(3) * 4 + 1)
            )
            .is_err()
    );
    assert_eq!(
        store
            .record::<DeviceUploads>(UPLOADS, "device")
            .unwrap()
            .unwrap()
            .uploads["small"]
            .received,
        0
    );
}

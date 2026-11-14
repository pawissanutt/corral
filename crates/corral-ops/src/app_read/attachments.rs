//! Attachment selectors resolve only message-owned canonical blobs, never paths or URLs.
use std::io::Read;

use anyhow::{Context, Result, ensure};
use corral_core::assistant::threads::Attachment;
use corral_protocol::{
    attachment::{ATTACHMENT_CHUNK, ATTACHMENT_IMAGE_MAX, ATTACHMENT_TEXT_MAX},
    detail::{ArtifactEncoding, ArtifactPage, DetailBody},
};

use crate::Ctx;

pub(super) fn select(ctx: &Ctx, entry: &str, id: &str) -> Result<Attachment> {
    let message = ctx
        .store
        .app_message(entry)
        .context("not_found: attachment message")?;
    let attachment = message
        .attachments
        .into_iter()
        .find(|item| item.id == id)
        .context("not_found: message attachment")?;
    let cap = match attachment.kind.as_str() {
        "image" => ATTACHMENT_IMAGE_MAX,
        "file" => ATTACHMENT_TEXT_MAX,
        _ => anyhow::bail!("not_found: attachment has no bytes"),
    };
    ensure!(
        (1..=cap).contains(&attachment.bytes),
        "invalid attachment size"
    );
    let blob = attachment
        .blob
        .as_ref()
        .context("not_found: attachment blob")?;
    ensure!(
        attachment.sha256.as_deref() == Some(blob.hash().as_str()),
        "conflict: attachment digest differs from canonical blob"
    );
    Ok(attachment)
}

pub(super) fn page(
    ctx: &Ctx,
    entry: &str,
    id: &str,
    attachment: &Attachment,
    offset: usize,
) -> Result<(DetailBody, Option<usize>)> {
    let offset_u64 = u64::try_from(offset).context("invalid attachment offset")?;
    ensure!(
        offset_u64 < attachment.bytes && offset.is_multiple_of(ATTACHMENT_CHUNK),
        "invalid attachment page offset"
    );
    let blob = attachment
        .blob
        .as_ref()
        .context("not_found: attachment blob")?;
    let mut reader = ctx
        .store
        .blob_reader(blob.hash())?
        .context("not_found: attachment blob")?;
    let skipped = std::io::copy(&mut reader.by_ref().take(offset_u64), &mut std::io::sink())
        .context("reading canonical attachment prefix")?;
    ensure!(skipped == offset_u64, "conflict: attachment blob shortened");
    let mut bytes = Vec::with_capacity(ATTACHMENT_CHUNK + 1);
    reader
        .take((ATTACHMENT_CHUNK + 1) as u64)
        .read_to_end(&mut bytes)
        .context("reading canonical attachment page")?;
    let remaining = attachment.bytes - offset_u64;
    ensure!(
        bytes.len() as u64 == remaining.min((ATTACHMENT_CHUNK + 1) as u64),
        "conflict: attachment blob length changed"
    );
    bytes.truncate(ATTACHMENT_CHUNK);
    let end = offset + bytes.len();
    let more = (end as u64) < attachment.bytes;
    Ok((
        DetailBody::Artifact(ArtifactPage {
            task: entry.to_owned(),
            key: id.to_owned(),
            media_type: attachment.media_type.clone(),
            encoding: ArtifactEncoding::Base64,
            content: super::artifacts::base64(&bytes),
            truncated: more,
        }),
        more.then_some(end),
    ))
}

#[cfg(test)]
#[test]
fn readback_is_message_owned_chunked_and_digest_bound() -> Result<()> {
    use corral_core::{Store, blob_ref::BlobHash};
    use corral_protocol::detail::{DetailGet, DetailRef};
    use serde_json::json;

    let directory = tempfile::tempdir()?;
    let ctx = Ctx::from_store(Store::init(directory.path())?);
    let bytes: Vec<_> = (0..ATTACHMENT_CHUNK + 5).map(|n| (n % 251) as u8).collect();
    let digest = BlobHash::of(&bytes).to_string();
    let entry = "A-001/M1";
    ctx.store
        .put_evidence("app-thread-messages", entry, &bytes, |blob| {
            json!({
                "id": entry, "session": "A-001", "seq": 1, "cmd_id": "send", "device": "one",
                "name": "owner", "target": "assistant", "text": "typed", "typed_at": super::now(),
                "at": super::now(), "state": "sent", "context": [], "rev": 1,
                "attachments": [{"id":"a1", "kind":"file", "name":"log.txt", "bytes":bytes.len(),
                    "sha256":digest, "blob":blob, "media_type":"text/plain"}]
            })
        })?;
    let mut request = DetailGet {
        target: DetailRef::Attachment {
            entry: entry.into(),
            id: "a1".into(),
        },
        etag: None,
        page: None,
    };
    let first = super::read(&ctx, &request)?;
    assert_eq!(first.etag, digest);
    assert!(serde_json::to_vec(&first)?.len() < 60 * 1024);
    let DetailBody::Artifact(first_body) = first.body.context("first body")? else {
        panic!("artifact body")
    };
    assert_eq!(
        first_body.content,
        super::artifacts::base64(&bytes[..ATTACHMENT_CHUNK])
    );
    request.page = first.next;
    let last = super::read(&ctx, &request)?;
    assert!(last.next.is_none());
    let DetailBody::Artifact(last_body) = last.body.context("last body")? else {
        panic!("artifact body")
    };
    assert_eq!(
        last_body.content,
        super::artifacts::base64(&bytes[ATTACHMENT_CHUNK..])
    );
    request.target = DetailRef::Attachment {
        entry: "A-001/M2".into(),
        id: "a1".into(),
    };
    assert!(super::read(&ctx, &request).is_err());
    request.target = DetailRef::Attachment {
        entry: entry.into(),
        id: "a2".into(),
    };
    request.page = None;
    assert!(super::read(&ctx, &request).is_err());
    Ok(())
}

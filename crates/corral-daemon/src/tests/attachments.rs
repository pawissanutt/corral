use std::{
    fs::{self, File},
    time::{Duration, SystemTime},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_protocol::{assistant::SendAttachment, attachment::*};
use sha2::{Digest, Sha256};

use super::Fixture;
use crate::assistant::{attach_begin, attach_discard, attach_put, attachments::prepare};

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join(crate::STATE_DIR).join("corral.toml"),
        "[profiles.pi]\nharness = 'pi'\n",
    )
    .unwrap();
    fixture
}

fn image(upload: impl Into<String>, bytes: u64) -> AttachmentBegin {
    AttachmentBegin {
        upload: upload.into(),
        kind: AttachmentKind::Image,
        name: "image.png".into(),
        media_type: "image/png".into(),
        bytes,
        sha256: "ab".repeat(32),
    }
}

#[test]
fn attachment_staging_exceeds_former_quotas_and_prunes_expired_uploads() {
    let fixture = fixture();
    let shared = fixture.shared();
    let dir = fixture
        .root()
        .join(crate::STATE_DIR)
        .join("chat/uploads/local");
    attach_begin(shared, "local", image("old", 1)).unwrap();
    File::open(dir.join("old"))
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(UPLOAD_TTL_SECS + 60))
        .unwrap();

    // The 9th declaration passes the old count cap; the 33rd passes 64 MiB.
    // Only metadata is persisted here, so the test does not need large files.
    let mut total = 0;
    for index in 1..=33 {
        let request = image(format!("image-{index}"), ATTACHMENT_IMAGE_MAX);
        let ack = attach_begin(shared, "local", request.clone()).unwrap();
        assert_eq!(ack.upload, request.upload);
        assert_eq!(ack.received, 0);
        assert!(!ack.ready);
        assert!(!ack.replayed);
        total += ack.bytes;
    }
    assert!(total > 64 * 1024 * 1024);
    assert!(!dir.join("old").exists());
    assert_eq!(fs::read_dir(dir).unwrap().count(), 33);
    assert!(
        attach_begin(shared, "local", image("image-1", ATTACHMENT_IMAGE_MAX))
            .unwrap()
            .replayed
    );
    let error =
        attach_begin(shared, "local", image("too-big", ATTACHMENT_IMAGE_MAX + 1)).unwrap_err();
    assert!(error.to_string().contains("size limit"));
}

#[test]
fn attachment_upload_still_enforces_ownership_and_digest() {
    let fixture = fixture();
    let shared = fixture.shared();
    let bytes = b"hello";
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let upload = AttachmentBegin {
        upload: "note".into(),
        kind: AttachmentKind::File,
        name: "note.txt".into(),
        media_type: "text/plain".into(),
        bytes: bytes.len() as u64,
        sha256: sha256.clone(),
    };
    attach_begin(shared, "owner", upload.clone()).unwrap();
    let put = |data: &[u8]| AttachmentPut {
        upload: upload.upload.clone(),
        offset: 0,
        data: STANDARD.encode(data),
    };
    let chat = fixture.root().join("session");
    let attachment = [SendAttachment::Upload {
        upload: upload.upload.clone(),
        sha256: sha256.clone(),
    }];
    assert!(attach_put(shared, "other", put(bytes)).is_err());
    let error = attach_put(shared, "owner", put(b"not it")).unwrap_err();
    assert!(error.to_string().contains("exceeds declared bytes"));
    let error = attach_put(shared, "owner", put(b"wrong")).unwrap_err();
    assert!(error.to_string().contains("digest does not match"));
    assert!(prepare(shared, "owner", &chat, &attachment).is_err());
    assert!(attach_put(shared, "owner", put(bytes)).unwrap().ready);
    assert!(attach_put(shared, "owner", put(bytes)).unwrap().replayed);
    assert!(prepare(shared, "other", &chat, &attachment).is_err());
    let kept = prepare(shared, "owner", &chat, &attachment).unwrap();
    assert_eq!(kept[0]["id"], "a1");
    attach_discard(
        shared,
        "owner",
        AttachmentDiscard {
            upload: upload.upload,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(chat.join("runtime/attachments").join(sha256)).unwrap(),
        bytes
    );
}

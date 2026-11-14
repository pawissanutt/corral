//! Device-owned bounded uploads. Lock order: app-threads, then app-attachments.
//! Ready blobs are referenced here until a message takes the reference in one transaction.
use super::threads::Attachment;
use crate::{
    Store,
    backend::{PageReq, RecordQuery},
    blob_ref::{BlobHash, BlobRef},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ATTACHMENTS_MAX: usize = 4;
pub const ATTACHMENT_CHUNK: usize = 36 * 1024;
const IMAGE_MAX: u64 = 2 * 1024 * 1024;
const TEXT_MAX: u64 = 64 * 1024;
const UPLOADS_MAX: usize = 8;
const DAY_MAX: u64 = 64 * 1024 * 1024;
const TTL: i64 = 24 * 60 * 60;
pub(crate) const UPLOADS: &str = "app-attachment-uploads";
const PARTS: &str = "app-attachment-parts";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upload {
    pub kind: String,
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
    pub sha256: String,
    pub received: u64,
    pub created: String,
    pub blob: Option<BlobRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceUploads {
    pub uploads: BTreeMap<String, Upload>,
    pub day: String,
    pub day_bytes: u64,
    // Retain chunk hashes after raw parts are deleted, for final-put replay.
    #[serde(default)]
    chunks: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct Part {
    data: String,
}

pub(crate) fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
fn part_key(device: &str, upload: &str, n: u64) -> String {
    format!("{device}/{upload}/{n:04}")
}
fn day(now: jiff::Timestamp) -> String {
    (now.as_second().div_euclid(TTL)).to_string()
}
fn expired(upload: &Upload, now: jiff::Timestamp) -> bool {
    upload
        .created
        .parse::<jiff::Timestamp>()
        .is_ok_and(|at| now.as_second().saturating_sub(at.as_second()) >= TTL)
}
fn validate(upload: &Upload) -> Result<()> {
    ensure!(
        !upload.name.is_empty()
            && upload.name.len() <= 160
            && !upload.name.chars().any(char::is_control),
        "invalid_params: invalid attachment name"
    );
    BlobHash::parse(&upload.sha256).context("invalid_params: invalid attachment digest")?;
    ensure!(
        !upload.media_type.is_empty()
            && upload.media_type.len() <= 160
            && !upload.media_type.chars().any(char::is_control),
        "invalid_params: invalid media type"
    );
    let cap = match upload.kind.as_str() {
        "image" => {
            ensure!(
                matches!(
                    upload.media_type.as_str(),
                    "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                ),
                "attachment_type_unsupported"
            );
            IMAGE_MAX
        }
        "file" => {
            ensure!(
                upload.media_type.starts_with("text/")
                    || matches!(
                        upload.media_type.as_str(),
                        "application/json" | "application/toml" | "application/yaml"
                    ),
                "attachment_type_unsupported"
            );
            TEXT_MAX
        }
        _ => bail!("attachment_type_unsupported"),
    };
    ensure!(
        upload.bytes > 0 && upload.bytes <= cap,
        "attachment_too_large"
    );
    Ok(())
}
fn valid_content(upload: &Upload, bytes: &[u8]) -> bool {
    match upload.kind.as_str() {
        "file" => !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok(),
        "image" => match upload.media_type.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            "image/webp" => {
                bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
            }
            _ => false,
        },
        _ => false,
    }
}

impl Store {
    pub fn app_upload_begin(
        &self,
        device: &str,
        upload: &str,
        mut new: Upload,
    ) -> Result<(Upload, bool)> {
        ensure!(
            key(device) && key(upload),
            "invalid_params: invalid upload identity"
        );
        validate(&new)?;
        let _lock = self.lock("app-attachments")?;
        let now = self.now();
        let mut state = self
            .record::<DeviceUploads>(UPLOADS, device)?
            .unwrap_or_default();
        // Clean this device without any area scan, even when the quota is full.
        self.prune_app_uploads(device, &mut state, now, false)?;
        if let Some(prior) = state.uploads.get(upload) {
            ensure!(
                prior.kind == new.kind
                    && prior.name == new.name
                    && prior.media_type == new.media_type
                    && prior.bytes == new.bytes
                    && prior.sha256 == new.sha256,
                "payload_mismatch: upload key already used"
            );
            return Ok((prior.clone(), true));
        }
        if state.day != day(now) {
            state.day = day(now);
            state.day_bytes = 0;
        }
        ensure!(
            state.uploads.len() < UPLOADS_MAX
                && new.bytes <= DAY_MAX.saturating_sub(state.day_bytes),
            "attachment_quota"
        );
        state.day_bytes += new.bytes;
        new.created = now.to_string();
        new.received = 0;
        new.blob = None;
        state.uploads.insert(upload.into(), new.clone());
        state.chunks.insert(upload.into(), Vec::new());
        self.save_record(UPLOADS, device, &state)?;
        Ok((new, false))
    }

    pub fn app_upload_put(
        &self,
        device: &str,
        upload: &str,
        offset: u64,
        data_b64: &str,
    ) -> Result<(Upload, bool)> {
        ensure!(
            key(device) && key(upload),
            "invalid_params: invalid upload identity"
        );
        // Validate the declared envelope before any byte allocation or record lookup.
        ensure!(
            offset.is_multiple_of(ATTACHMENT_CHUNK as u64)
                && !data_b64.is_empty()
                && data_b64.len() <= ATTACHMENT_CHUNK.div_ceil(3) * 4,
            "offset_mismatch: invalid chunk bounds"
        );
        let _lock = self.lock("app-attachments")?;
        let mut state = self
            .record::<DeviceUploads>(UPLOADS, device)?
            .unwrap_or_default();
        let mut item = state
            .uploads
            .get(upload)
            .cloned()
            .context("attachment_unknown")?;
        validate(&item)?;
        if expired(&item, self.now()) {
            self.remove_app_upload(device, upload, &mut state)?;
            bail!("attachment_expired");
        }
        ensure!(offset < item.bytes, "offset_mismatch");
        let expected = (item.bytes - offset).min(ATTACHMENT_CHUNK as u64) as usize;
        ensure!(
            data_b64.len() == expected.div_ceil(3) * 4,
            "offset_mismatch: invalid chunk length"
        );
        let data = STANDARD
            .decode(data_b64)
            .context("invalid_params: invalid padded base64")?;
        ensure!(
            data.len() == expected,
            "offset_mismatch: invalid chunk length"
        );
        let chunk_hash = BlobHash::of(&data).to_string();
        let n = offset / ATTACHMENT_CHUNK as u64;
        if offset < item.received {
            ensure!(
                state
                    .chunks
                    .get(upload)
                    .and_then(|chunks| chunks.get(n as usize))
                    == Some(&chunk_hash),
                "offset_mismatch: chunk differs from accepted bytes"
            );
            // Also finish cleanup after an interrupted post-publication part deletion.
            if item.blob.is_some() {
                self.remove_app_parts(device, upload, item.bytes)?;
            }
            return Ok((item, true));
        }
        ensure!(
            offset == item.received && item.blob.is_none(),
            "offset_mismatch"
        );
        let chunks = state.chunks.entry(upload.into()).or_default();
        ensure!(
            chunks.len() == n as usize,
            "offset_mismatch: corrupt upload progress"
        );
        chunks.push(chunk_hash);
        item.received += data.len() as u64;
        if item.received < item.bytes {
            state.uploads.insert(upload.into(), item.clone());
            self.records_transaction(|| {
                self.save_record(
                    PARTS,
                    &part_key(device, upload, n),
                    &Part {
                        data: data_b64.into(),
                    },
                )?;
                self.save_record(UPLOADS, device, &state)
            })?;
            return Ok((item, false));
        }
        // Declared length was checked against the kind cap before allocating this buffer.
        let mut bytes = Vec::with_capacity(item.bytes as usize);
        for prior in 0..n {
            let part: Part = self
                .record(PARTS, &part_key(device, upload, prior))?
                .context("attachment_unknown: missing chunk")?;
            ensure!(
                part.data.len() == ATTACHMENT_CHUNK.div_ceil(3) * 4,
                "offset_mismatch: corrupt chunk length"
            );
            let before = bytes.len();
            STANDARD
                .decode_vec(&part.data, &mut bytes)
                .context("decoding retained attachment chunk")?;
            ensure!(
                bytes.len() - before == ATTACHMENT_CHUNK,
                "offset_mismatch: corrupt chunk length"
            );
        }
        bytes.extend_from_slice(&data);
        let failure = if BlobHash::of(&bytes).as_str() != item.sha256 {
            Some("digest_mismatch")
        } else if !valid_content(&item, &bytes) {
            Some("attachment_type_unsupported")
        } else {
            None
        };
        if let Some(failure) = failure {
            self.remove_app_upload(device, upload, &mut state)?;
            bail!("{failure}");
        }
        // A live reference is committed with the blob reservation. No unreferenced gap.
        self.put_evidence(UPLOADS, device, &bytes, |blob| {
            item.blob = Some(blob.clone());
            state.uploads.insert(upload.into(), item.clone());
            serde_json::json!(state)
        })?;
        self.remove_app_parts(device, upload, item.bytes)?;
        Ok((item, false))
    }

    pub fn app_upload_discard(&self, device: &str, upload: &str) -> Result<()> {
        ensure!(
            key(device) && key(upload),
            "invalid_params: invalid upload identity"
        );
        let _lock = self.lock("app-attachments")?;
        let mut state = self
            .record::<DeviceUploads>(UPLOADS, device)?
            .unwrap_or_default();
        self.remove_app_upload(device, upload, &mut state)
    }

    /// Maintenance reads one device page at a time, never all uploaded bytes or history.
    pub fn app_upload_sweep(&self, live_device: impl Fn(&str) -> bool) -> Result<()> {
        let _lock = self.lock("app-attachments")?;
        let now = self.now();
        let mut page = PageReq::first(64);
        loop {
            let found = self.record_view()?.records(
                &RecordQuery {
                    area: Some(UPLOADS.into()),
                    key: None,
                },
                page.clone(),
            )?;
            for row in found.items {
                let mut state: DeviceUploads =
                    serde_json::from_value(row.doc).context("decoding device uploads")?;
                let revoked = !live_device(&row.key);
                self.prune_app_uploads(&row.key, &mut state, now, revoked)?;
                // Keep today's quota even when every upload was consumed/discarded.
                if state.uploads.is_empty() && (state.day != day(now) || revoked) {
                    self.delete_record(UPLOADS, &row.key)?;
                }
            }
            match found.next {
                Some(next) => page.after = Some(next),
                None => break,
            }
        }
        Ok(())
    }

    fn remove_app_parts(&self, device: &str, upload: &str, bytes: u64) -> Result<()> {
        ensure!(bytes <= IMAGE_MAX, "attachment_too_large");
        self.records_transaction(|| {
            for n in 0..bytes.div_ceil(ATTACHMENT_CHUNK as u64) {
                self.delete_record(PARTS, &part_key(device, upload, n))?;
            }
            Ok(())
        })
    }

    fn remove_app_upload(
        &self,
        device: &str,
        upload: &str,
        state: &mut DeviceUploads,
    ) -> Result<()> {
        let Some(item) = state.uploads.remove(upload) else {
            return Ok(());
        };
        state.chunks.remove(upload);
        self.records_transaction(|| {
            self.remove_app_parts(device, upload, item.bytes)?;
            self.save_record(UPLOADS, device, state)
        })
    }

    fn prune_app_uploads(
        &self,
        device: &str,
        state: &mut DeviceUploads,
        now: jiff::Timestamp,
        revoked: bool,
    ) -> Result<()> {
        let expired: Vec<_> = state
            .uploads
            .iter()
            .filter(|(_, item)| revoked || expired(item, now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.remove_app_upload(device, &id, state)?;
        }
        Ok(())
    }

    /// Called with both attachment/thread locks held, inside the message transaction.
    pub(crate) fn take_app_uploads(
        &self,
        device: &str,
        refs: &[(String, String)],
    ) -> Result<Vec<(String, Attachment)>> {
        ensure!(
            refs.len() <= ATTACHMENTS_MAX,
            "invalid_params: too many attachments"
        );
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        let mut state = self
            .record::<DeviceUploads>(UPLOADS, device)?
            .unwrap_or_default();
        let mut taken = Vec::with_capacity(refs.len());
        for (id, digest) in refs {
            ensure!(key(id), "invalid_params: invalid upload identity");
            let item = state.uploads.remove(id).context("attachment_unknown")?;
            ensure!(!expired(&item, self.now()), "attachment_expired");
            ensure!(
                &item.sha256 == digest,
                "payload_mismatch: upload digest differs"
            );
            ensure!(
                item.blob.is_some() && item.received == item.bytes,
                "attachment_unknown: upload is not ready"
            );
            validate(&item)?;
            ensure!(
                item.blob
                    .as_ref()
                    .is_some_and(|blob| blob.hash().as_str() == item.sha256),
                "payload_mismatch: blob digest differs"
            );
            state.chunks.remove(id);
            self.remove_app_parts(device, id, item.bytes)?;
            taken.push((
                id.clone(),
                Attachment {
                    id: String::new(),
                    kind: item.kind,
                    name: item.name,
                    media_type: Some(item.media_type),
                    bytes: item.bytes,
                    sha256: Some(item.sha256),
                    blob: item.blob,
                    url: None,
                },
            ));
        }
        if !refs.is_empty() {
            self.save_record(UPLOADS, device, &state)?;
        }
        Ok(taken)
    }
}

#[cfg(test)]
mod tests;

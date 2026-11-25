//! Authenticated uploads with per-file bounds and digest-addressed delivery files.
use super::{AuthenticatedDevice, input, valid_key};
use crate::Ctx;
use anyhow::{Context, Result, bail, ensure};
use corral_core::assistant::{
    attachments::Upload,
    threads::{Attachment, Message},
};
use corral_protocol::assistant::SendAttachment;
use corral_protocol::attachment::*;
use serde_json::{Value, json};
use std::collections::HashSet;

fn authorize(ctx: &Ctx, device: &AuthenticatedDevice) -> Result<()> {
    input::recheck_authority(&ctx.store, &device.id, false)?;
    ensure!(device.assistant, "authority_assistant");
    ensure!(
        valid_key(&device.id),
        "invalid_params: invalid device identity"
    );
    Ok(())
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn name(value: &str) -> bool {
    value.len() <= ATTACHMENT_NAME_MAX && !value.chars().any(char::is_control)
}

pub fn valid_link(url: &str) -> bool {
    if url.len() > ATTACHMENT_URL_MAX || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty() && !authority.contains('@') && !url.contains('\\')
}

pub fn begin(
    ctx: &Ctx,
    device: &AuthenticatedDevice,
    request: &AttachmentBegin,
) -> Result<UploadAck> {
    authorize(ctx, device)?;
    ensure!(
        valid_key(&request.upload)
            && digest(&request.sha256)
            && name(&request.name)
            && !request.name.is_empty(),
        "invalid_params: invalid attachment metadata"
    );
    let (kind, cap) = match request.kind {
        AttachmentKind::Image => {
            ensure!(
                matches!(
                    request.media_type.as_str(),
                    "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                ),
                "attachment_type_unsupported"
            );
            ("image", ATTACHMENT_IMAGE_MAX)
        }
        AttachmentKind::File => {
            ensure!(
                request.media_type == "text/plain"
                    || request.media_type.starts_with("text/")
                    || matches!(
                        request.media_type.as_str(),
                        "application/json" | "application/toml" | "application/yaml"
                    ),
                "attachment_type_unsupported"
            );
            ("file", ATTACHMENT_TEXT_MAX)
        }
        _ => bail!("attachment_type_unsupported"),
    };
    ensure!(
        request.media_type.len() <= 160 && !request.media_type.chars().any(char::is_control),
        "invalid_params: invalid media type"
    );
    ensure!(
        request.bytes > 0 && request.bytes <= cap,
        "attachment_too_large"
    );
    let (upload, replayed) = ctx.store.app_upload_begin(
        &device.id,
        &request.upload,
        Upload {
            kind: kind.into(),
            name: request.name.clone(),
            media_type: request.media_type.clone(),
            bytes: request.bytes,
            sha256: request.sha256.clone(),
            received: 0,
            created: String::new(),
            blob: None,
        },
    )?;
    Ok(ack(&request.upload, &upload, replayed))
}

fn ack(id: &str, upload: &Upload, replayed: bool) -> UploadAck {
    UploadAck {
        upload: id.into(),
        received: upload.received,
        bytes: upload.bytes,
        ready: upload.blob.is_some(),
        replayed,
    }
}

pub fn put(ctx: &Ctx, device: &AuthenticatedDevice, request: &AttachmentPut) -> Result<UploadAck> {
    authorize(ctx, device)?;
    ensure!(
        valid_key(&request.upload),
        "invalid_params: invalid upload identity"
    );
    ensure!(
        request.offset.is_multiple_of(ATTACHMENT_CHUNK as u64)
            && !request.data.is_empty()
            && request.data.len() <= ATTACHMENT_CHUNK.div_ceil(3) * 4,
        "offset_mismatch: invalid attachment chunk bounds"
    );
    let (upload, replayed) =
        ctx.store
            .app_upload_put(&device.id, &request.upload, request.offset, &request.data)?;
    Ok(ack(&request.upload, &upload, replayed))
}

pub fn discard(ctx: &Ctx, device: &AuthenticatedDevice, request: &AttachmentDiscard) -> Result<()> {
    authorize(ctx, device)?;
    ensure!(
        valid_key(&request.upload),
        "invalid_params: invalid upload identity"
    );
    ctx.store.app_upload_discard(&device.id, &request.upload)
}

/// A message's attachments and the `(upload, sha256)` pairs they claim.
type References = (Vec<Attachment>, Vec<(String, String)>);

pub(super) fn references(requests: &[SendAttachment]) -> Result<References> {
    let mut attachments = Vec::with_capacity(requests.len());
    let mut uploads = Vec::new();
    let mut seen = HashSet::new();
    for request in requests {
        attachments.push(match request {
            SendAttachment::Upload { upload, sha256 } => {
                ensure!(
                    valid_key(upload) && digest(sha256) && seen.insert(upload.as_str()),
                    "invalid_params: invalid upload reference"
                );
                uploads.push((upload.clone(), sha256.clone()));
                Attachment {
                    id: upload.clone(),
                    kind: "upload".into(),
                    name: String::new(),
                    media_type: None,
                    bytes: 0,
                    sha256: None,
                    blob: None,
                    url: None,
                }
            }
            SendAttachment::Link { url, title } => {
                ensure!(
                    valid_link(url) && title.as_ref().is_none_or(|t| name(t)),
                    "invalid_params: invalid web link"
                );
                Attachment {
                    id: String::new(),
                    kind: "link".into(),
                    name: title.clone().unwrap_or_default(),
                    media_type: None,
                    bytes: 0,
                    sha256: None,
                    blob: None,
                    url: Some(url.clone()),
                }
            }
            _ => bail!("invalid_params: unsupported attachment reference"),
        });
    }
    Ok((attachments, uploads))
}

/// The retained directory handle keeps cleanup scoped to the directory that received files,
/// even if a path is replaced. Files are removed after Pi's synchronous pre-admission read.
pub(super) struct Materialized {
    pub metadata: Vec<Value>,
    #[cfg(unix)]
    directory: Option<std::fs::File>,
    #[cfg(unix)]
    names: Vec<String>,
}

#[cfg(unix)]
impl Drop for Materialized {
    fn drop(&mut self) {
        if let Some(directory) = &self.directory {
            for name in &self.names {
                let _ =
                    rustix::fs::unlinkat(directory, name.as_str(), rustix::fs::AtFlags::empty());
            }
        }
    }
}

pub(super) fn materialize(
    ctx: &Ctx,
    session: &str,
    generation: u32,
    message: &Message,
) -> Result<Materialized> {
    let mut result = Materialized {
        metadata: Vec::with_capacity(message.attachments.len()),
        #[cfg(unix)]
        directory: None,
        #[cfg(unix)]
        names: Vec::new(),
    };
    for attachment in &message.attachments {
        if attachment.kind != "link" {
            #[cfg(unix)]
            {
                if result.directory.is_none() {
                    use crate::agent::launch;
                    let path = launch::assistant_runtime_dir(&ctx.store, session)
                        .join(format!("g{generation}"))
                        .join("attachments");
                    result.directory = Some(open_directory(&launch::runtime_root(), &path)?);
                }
                write_blob(
                    ctx,
                    result.directory.as_ref().unwrap(),
                    attachment,
                    &mut result.names,
                )?;
            }
            #[cfg(not(unix))]
            bail!("attachment materialization requires Unix");
        }
        result
            .metadata
            .push(json!({"id": attachment.id, "kind": attachment.kind, "name": attachment.name}));
        let metadata = result.metadata.last_mut().unwrap();
        if let Some(value) = &attachment.media_type {
            metadata["media_type"] = json!(value);
        }
        if attachment.kind != "link" {
            metadata["bytes"] = json!(attachment.bytes);
        }
        if let Some(value) = &attachment.sha256 {
            metadata["sha256"] = json!(value);
        }
        if let Some(value) = &attachment.url {
            metadata["url"] = json!(value);
        }
    }
    Ok(result)
}

#[cfg(unix)]
fn open_directory(root: &std::path::Path, path: &std::path::Path) -> Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    crate::agent::launch::ensure_private_dir(root, path)?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory: std::fs::File = rustix::fs::open(root, flags, Mode::empty())?.into();
    for part in path.strip_prefix(root)?.components() {
        let std::path::Component::Normal(name) = part else {
            bail!("unsafe runtime directory component");
        };
        directory = rustix::fs::openat(&directory, name, flags, Mode::empty())?.into();
        let metadata = directory.metadata()?;
        ensure!(
            metadata.uid() == rustix::process::getuid().as_raw() && metadata.mode() & 0o077 == 0,
            "unsafe runtime attachment directory"
        );
    }
    Ok(directory)
}

#[cfg(unix)]
fn write_blob(
    ctx: &Ctx,
    directory: &std::fs::File,
    attachment: &Attachment,
    names: &mut Vec<String>,
) -> Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    use std::io::{Read, Write};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let hash = attachment
        .sha256
        .as_deref()
        .filter(|s| digest(s))
        .context("invalid stored attachment digest")?;
    let cap = match attachment.kind.as_str() {
        "image" => ATTACHMENT_IMAGE_MAX,
        "file" => ATTACHMENT_TEXT_MAX,
        _ => bail!("invalid stored attachment kind"),
    };
    ensure!(
        attachment.bytes > 0 && attachment.bytes <= cap,
        "invalid stored attachment size"
    );
    let blob = attachment
        .blob
        .as_ref()
        .context("attachment blob missing")?;
    ensure!(
        blob.hash().as_str() == hash,
        "attachment blob digest differs"
    );
    let mut bytes = Vec::with_capacity(attachment.bytes as usize);
    ctx.store
        .blob_reader(blob.hash())?
        .context("attachment blob unavailable")?
        .take(attachment.bytes + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == attachment.bytes
            && corral_core::agent::paths::sha256_hex(&bytes) == hash,
        "attachment blob length or digest differs"
    );
    let temporary = format!(
        ".{hash}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut file: std::fs::File = rustix::fs::openat(
        directory,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?
    .into();
    let written = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        rustix::fs::renameat(directory, temporary.as_str(), directory, hash)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = rustix::fs::unlinkat(directory, temporary.as_str(), AtFlags::empty());
    }
    written?;
    names.push(hash.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_remain_bounded_opaque_web_references() {
        assert!(valid_link("https://example.com/a?b=c#d"));
        for url in [
            "file:///etc/passwd",
            "https://user:pass@example.com",
            "https:///empty",
            "http://x/\n",
            "https://x\\@private",
        ] {
            assert!(!valid_link(url), "{url:?}");
        }
        assert!(!valid_link(&format!(
            "https://example.com/{}",
            "a".repeat(ATTACHMENT_URL_MAX)
        )));
    }
}

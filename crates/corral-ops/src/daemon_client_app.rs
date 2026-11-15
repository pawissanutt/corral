//! Authenticated native app commands for local CLI clients.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use corral_core::Store;
use corral_protocol::assistant::{AssistantSend, SendAck, SendAttachment};
use corral_protocol::attachment::*;
use corral_protocol::device::*;
use fs2::FileExt;
use sha2::{Digest, Sha256};

use super::{
    Client, ClientInfo, Hello, PROTO_MAJOR, PROTO_MINOR, Proto, Welcome, check_peer,
    connect_socket, socket_path,
};

pub struct AppClient {
    client: Client,
    attachments: bool,
}

impl AppClient {
    pub fn connect(store: &Store) -> Result<Self> {
        let socket = socket_path(store);
        let stream = connect_socket(&socket, Instant::now() + Duration::from_secs(2))
            .with_context(|| format!("connecting to daemon at {}", socket.display()))?;
        check_peer(&stream)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut client = Client {
            stream,
            next_id: 0,
            read_capable: false,
            cmd_capable: false,
            classic_capable: false,
            device_capable: false,
        };
        let welcome: Welcome = client.request_with_timeout(
            corral_protocol::HELLO,
            Hello {
                proto: Proto {
                    major: PROTO_MAJOR,
                    minor: PROTO_MINOR,
                },
                caps: vec![
                    DEVICE_V1.into(),
                    corral_protocol::app::ASSISTANT_V1.into(),
                    ASSISTANT_ATTACH_V1.into(),
                ],
                client: ClientInfo {
                    kind: "app".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            },
            Duration::from_secs(2),
        )?;
        ensure!(
            welcome.proto.major == PROTO_MAJOR,
            "unsupported daemon protocol version"
        );
        ensure!(
            welcome.caps.iter().any(|c| c == DEVICE_V1),
            "daemon does not support device enrolment"
        );
        ensure!(
            welcome
                .caps
                .iter()
                .any(|c| c == corral_protocol::app::ASSISTANT_V1),
            "daemon does not support Ask"
        );
        let attachments = welcome.caps.iter().any(|c| c == ASSISTANT_ATTACH_V1);
        let (_lock, path) = identity_path(&welcome.daemon.store)?;
        match read_identity(&path)? {
            Some(device) => {
                let _: DeviceInfo = client.request_with_timeout(DEVICE_ATTACH, DeviceAttach { device }, Duration::from_secs(30))
                    .context("attaching CLI device (if revoked, remove its local identity before enrolling again)")?;
            }
            None => {
                let info: DeviceInfo = client.request_with_timeout(
                    DEVICE_ENROL,
                    DeviceEnrol {
                        name: "corral CLI".into(),
                        app_version: env!("CARGO_PKG_VERSION").into(),
                    },
                    Duration::from_secs(30),
                )?;
                let mut file = tempfile::NamedTempFile::new_in(
                    path.parent().context("invalid CLI device identity path")?,
                )
                .context("creating private CLI device identity")?;
                file.write_all(info.id.as_bytes())
                    .context("saving CLI device identity")?;
                file.as_file()
                    .sync_all()
                    .context("syncing CLI device identity")?;
                file.persist_noclobber(&path)
                    .context("publishing CLI device identity")?;
            }
        }
        Ok(Self {
            client,
            attachments,
        })
    }

    pub fn send(&mut self, request: &AssistantSend) -> Result<SendAck> {
        self.client
            .request_with_timeout(
                corral_protocol::assistant::ASSISTANT_SEND,
                request,
                Duration::from_secs(30),
            )
            .context("sending Ask message")
    }

    pub fn upload(&mut self, path: &Path) -> Result<SendAttachment> {
        ensure!(self.attachments, "daemon does not support Ask attachments");
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("attachment needs a UTF-8 file name")?;
        let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(ATTACHMENT_IMAGE_MAX + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {}", path.display()))?;
        let (kind, media, limit) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            (AttachmentKind::Image, "image/png", ATTACHMENT_IMAGE_MAX)
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            (AttachmentKind::Image, "image/jpeg", ATTACHMENT_IMAGE_MAX)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            (AttachmentKind::Image, "image/gif", ATTACHMENT_IMAGE_MAX)
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            (AttachmentKind::Image, "image/webp", ATTACHMENT_IMAGE_MAX)
        } else {
            std::str::from_utf8(&bytes).context("attachment must be an image or UTF-8 text")?;
            ensure!(!bytes.contains(&0), "text attachment contains NUL");
            (AttachmentKind::File, "text/plain", ATTACHMENT_TEXT_MAX)
        };
        ensure!(
            !bytes.is_empty() && bytes.len() as u64 <= limit,
            "attachment is empty or exceeds the size limit"
        );
        let upload = new_id()?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let mut ack: UploadAck = self.client.request_with_timeout(
            ATTACHMENT_BEGIN,
            AttachmentBegin {
                upload: upload.clone(),
                kind,
                name: name.into(),
                media_type: media.into(),
                bytes: bytes.len() as u64,
                sha256: sha256.clone(),
            },
            Duration::from_secs(30),
        )?;
        while !ack.ready {
            let offset = usize::try_from(ack.received).context("invalid upload offset")?;
            ensure!(
                offset < bytes.len(),
                "daemon returned invalid upload progress"
            );
            let end = (offset + ATTACHMENT_CHUNK).min(bytes.len());
            ack = self.client.request_with_timeout(
                ATTACHMENT_PUT,
                AttachmentPut {
                    upload: upload.clone(),
                    offset: offset as u64,
                    data: base64(&bytes[offset..end]),
                },
                Duration::from_secs(30),
            )?;
            ensure!(
                ack.received > offset as u64,
                "daemon did not advance upload"
            );
        }
        Ok(SendAttachment::Upload { upload, sha256 })
    }
}

pub fn new_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("generating command identity: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn identity_path(store: &str) -> Result<(fs::File, PathBuf)> {
    let root = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|p| PathBuf::from(p).join(".local/state"))
        })
        .context("set XDG_STATE_HOME or HOME to store the CLI device identity")?;
    fs::create_dir_all(&root).context("creating state directory")?;
    let dir = root.join("corral-cli");
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e).context("creating CLI state directory"),
    }
    let metadata = fs::symlink_metadata(&dir)?;
    ensure!(
        metadata.is_dir()
            && metadata.mode() & 0o077 == 0
            && metadata.uid() == rustix::process::getuid().as_raw(),
        "CLI state directory must be private and owned by you"
    );
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(dir.join("devices.lock"))?;
    let meta = lock.metadata()?;
    ensure!(
        meta.is_file()
            && meta.mode() & 0o077 == 0
            && meta.uid() == rustix::process::getuid().as_raw(),
        "CLI device lock must be private and owned by you"
    );
    lock.lock_exclusive()
        .context("locking CLI device identity")?;
    let key = format!("{:x}", Sha256::digest(store.as_bytes()));
    Ok((lock, dir.join(key)))
}

fn read_identity(path: &Path) -> Result<Option<String>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("opening CLI device identity"),
    };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.mode() & 0o077 == 0
            && meta.uid() == rustix::process::getuid().as_raw(),
        "CLI device identity must be private and owned by you"
    );
    let mut value = String::new();
    file.take(1025).read_to_string(&mut value)?;
    if value.is_empty() || value.len() > 1024 {
        bail!("invalid CLI device identity in {}", path.display());
    }
    Ok(Some(value))
}

use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use corral_protocol::rpc::ErrorCode;
use sha2::{Digest, Sha256};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

// Picker bytes remain Arc-shared. Native file reads own one bounded allocation.
enum Bytes {
    Picked(nsh_core::image::ImageData),
    #[cfg(any(test, not(target_arch = "wasm32")))]
    Local(Vec<u8>),
}
impl Bytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Picked(data) => data,
            #[cfg(any(test, not(target_arch = "wasm32")))]
            Self::Local(data) => data,
        }
    }
}
pub(super) struct Upload {
    begin: AttachmentBegin,
    data: Option<Bytes>,
    received: u64,
    begun: bool,
    ready: bool,
    failed: bool,
    inflight: Option<String>,
}
impl Upload {
    pub(super) fn disconnected(&mut self) {
        self.inflight = None;
        self.begun = false;
    }
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn accepts(&self, tag: &Option<String>) -> bool {
        tag.is_some() && self.inflight.as_ref() == tag.as_ref()
    }
}
impl AppState {
    pub(super) fn attach(
        &mut self,
        feed: Signal<Feed>,
        request: requests::AttachRequest,
    ) -> Vec<Outgoing> {
        self.attach_bytes(
            feed,
            request.upload,
            request.kind,
            request.name,
            request.media_type,
            Bytes::Picked(request.bytes),
        )
    }
    fn attach_bytes(
        &mut self,
        feed: Signal<Feed>,
        id: String,
        kind: AttachmentKind,
        name: String,
        media_type: String,
        bytes: Bytes,
    ) -> Vec<Outgoing> {
        if self.uploads.contains_key(&id)
            || feed.with_untracked(|feed| feed.uploads.contains_key(id.as_str()))
        {
            feed.update(|feed| {
                feed.problem = Some("Attachment identity already belongs to a file.".into())
            });
            return vec![];
        }
        let data = bytes.as_slice();
        // File MIME labels from pickers vary by extension; UTF-8 content is the canonical type.
        let media_type = if kind == AttachmentKind::File {
            "text/plain".into()
        } else {
            media_type
        };
        let error = if !self.ready {
            Some(offline())
        } else if !feed.with_untracked(|feed| feed.capable(ASSISTANT_ATTACH_V1)) {
            Some(failure(
                "attachment_unsupported",
                "corrald does not accept attachments; upgrade corral",
            ))
        } else {
            validate_bytes(&kind, &name, &media_type, data).err()
        };
        let mut state = UploadState {
            kind: kind.clone(),
            name: name.clone(),
            media_type: media_type.clone(),
            bytes: data.len() as u64,
            sent: 0,
            sha256: String::new(),
            status: UploadStatus::Uploading,
        };
        if let Some(error) = error {
            state.status = UploadStatus::Failed(error);
            feed.update(|feed| {
                feed.uploads.insert(id.into(), state);
            });
            return vec![];
        }
        state.sha256 = format!("{:x}", Sha256::digest(data));
        let begin = AttachmentBegin {
            upload: id.clone(),
            kind,
            name,
            media_type,
            bytes: state.bytes,
            sha256: state.sha256.clone(),
        };
        self.uploads.insert(
            id.clone(),
            Upload {
                begin,
                data: Some(bytes),
                received: 0,
                begun: false,
                ready: false,
                failed: false,
                inflight: None,
            },
        );
        feed.update(|feed| {
            feed.uploads.insert(id.into(), state);
        });
        self.pump_uploads()
    }
    pub(super) fn pump_uploads(&mut self) -> Vec<Outgoing> {
        if !self.ready {
            return vec![];
        }
        let mut available = 2usize.saturating_sub(
            self.uploads
                .values()
                .filter(|upload| upload.inflight.is_some())
                .count(),
        );
        let mut outgoing = vec![];
        for upload in self.uploads.values_mut() {
            if available == 0 {
                break;
            }
            if upload.ready || upload.failed || upload.inflight.is_some() {
                continue;
            }
            let Some(data) = &upload.data else {
                continue;
            };
            let tag = requests::new_id();
            let request = if !upload.begun {
                out(ATTACHMENT_BEGIN, value(&upload.begin), Some(tag.clone()))
            } else {
                let offset = upload.received as usize;
                let end = (offset + ATTACHMENT_CHUNK).min(data.as_slice().len());
                out(
                    ATTACHMENT_PUT,
                    value(AttachmentPut {
                        upload: upload.begin.upload.clone(),
                        offset: upload.received,
                        data: STANDARD.encode(&data.as_slice()[offset..end]),
                    }),
                    Some(tag.clone()),
                )
            };
            upload.inflight = Some(tag);
            available -= 1;
            outgoing.push(request);
        }
        outgoing
    }
    pub(super) fn upload_reply(
        &mut self,
        feed: Signal<Feed>,
        method: &'static str,
        tag: Option<String>,
        params: Value,
        outcome: Result<Value, (ErrorCode, String)>,
        error_data: Option<Value>,
    ) -> Result<Vec<Outgoing>, String> {
        if method == ATTACHMENT_DISCARD {
            return Ok(vec![]);
        }
        let Some(id) = params["upload"].as_str() else {
            return Err("Upload reply has no upload identity.".into());
        };
        let Some(upload) = self.uploads.get_mut(id) else {
            return Ok(vec![]);
        };
        if upload.inflight.as_ref() != tag.as_ref() || tag.is_none() {
            return Ok(vec![]);
        }
        upload.inflight = None;
        match outcome {
            Err((code, message)) => {
                let code = error_data
                    .as_ref()
                    .and_then(|data| data["code"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| value(code).as_str().unwrap_or("unknown").into());
                if code == "offset_mismatch" && method == ATTACHMENT_PUT {
                    upload.begun = false;
                } else {
                    upload.failed = true;
                    upload.data = None;
                    feed.update(|feed| {
                        if let Some(state) = feed.uploads.get_mut(id) {
                            state.status = UploadStatus::Failed(ClientError { code, message });
                        }
                    });
                }
            }
            Ok(result) => {
                let ack: UploadAck = decode(result)?;
                if ack.upload != id
                    || ack.bytes != upload.begin.bytes
                    || ack.received > ack.bytes
                    || (ack.ready && ack.received != ack.bytes)
                    || (!ack.ready
                        && (ack.received == ack.bytes
                            || ack.received % ATTACHMENT_CHUNK as u64 != 0))
                {
                    return Err("Invalid upload acknowledgement boundary.".into());
                }
                if method == ATTACHMENT_PUT {
                    let expected = (params["offset"]
                        .as_u64()
                        .ok_or("Upload put lost its offset")?
                        + ATTACHMENT_CHUNK as u64)
                        .min(ack.bytes);
                    if ack.received != expected {
                        return Err(
                            "Upload acknowledgement did not advance exactly one chunk.".into()
                        );
                    }
                }
                upload.received = ack.received;
                upload.begun = true;
                upload.ready = ack.ready;
                feed.update(|feed| {
                    if let Some(state) = feed.uploads.get_mut(id) {
                        state.sent = ack.received;
                        state.status = if ack.ready {
                            UploadStatus::Ready
                        } else {
                            UploadStatus::Uploading
                        };
                    }
                });
            }
        }
        Ok(self.pump_uploads())
    }
    pub(super) fn discard_upload(&mut self, feed: Signal<Feed>, id: String) -> Vec<Outgoing> {
        self.uploads.remove(&id);
        feed.update(|feed| {
            feed.uploads.remove(id.as_str());
        });
        let mut outgoing = if self.ready {
            vec![out(
                ATTACHMENT_DISCARD,
                value(AttachmentDiscard { upload: id }),
                Some(requests::new_id()),
            )]
        } else {
            vec![]
        };
        outgoing.extend(self.pump_uploads());
        outgoing
    }
    pub(super) fn ready_uploads(&self, feed: Signal<Feed>, request: &SessionSend) -> bool {
        feed.with_untracked(|feed| {
            request.attachments.iter().all(|item| match item {
                SendAttachment::Upload { upload, sha256 } => {
                    feed.uploads.get(upload.as_str()).is_some_and(|state| {
                        state.status == UploadStatus::Ready && state.sha256 == *sha256
                    })
                }
                SendAttachment::Link { .. } => true,
                SendAttachment::Other => false,
            })
        })
    }
    pub(super) fn release_upload_bytes(&mut self, feed: Signal<Feed>, request: &SessionSend) {
        feed.update(|feed| {
            for item in &request.attachments {
                if let SendAttachment::Upload { upload, .. } = item {
                    self.uploads.remove(upload);
                    feed.uploads.remove(upload.as_str());
                }
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn attach_path(
        &mut self,
        feed: Signal<Feed>,
        request: requests::AttachPath,
    ) -> Vec<Outgoing> {
        let name = local_name(&request.path);
        if feed.with_untracked(|feed| feed.uploads.contains_key(request.upload.as_str())) {
            return vec![];
        }
        let id = Id::from(&request.upload);
        let error = (!self.ready).then(offline);
        feed.update(|feed| {
            feed.uploads.insert(
                id.clone(),
                UploadState {
                    kind: request.kind.clone(),
                    name: name.clone(),
                    media_type: String::new(),
                    bytes: 0,
                    sent: 0,
                    sha256: String::new(),
                    status: error
                        .clone()
                        .map(UploadStatus::Failed)
                        .unwrap_or(UploadStatus::Uploading),
                },
            );
        });
        if error.is_some() {
            return vec![];
        }
        if enqueue_local_read(request, name).is_err() {
            feed.update(|feed| {
                if let Some(state) = feed.uploads.get_mut(id.as_str()) {
                    state.status = UploadStatus::Failed(failure(
                        "local_read_failed",
                        "Cannot start a local file reader; try again.",
                    ));
                }
            });
        }
        vec![]
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn attach_loaded(
        &mut self,
        feed: Signal<Feed>,
        id: String,
        kind: AttachmentKind,
        name: String,
        result: Result<(Vec<u8>, String), ClientError>,
    ) -> Vec<Outgoing> {
        // A discarded chip or switched device must not be resurrected by a slow disk read.
        if !feed.with_untracked(|feed| {
            feed.uploads
                .get(id.as_str())
                .is_some_and(|state| state.status == UploadStatus::Uploading)
        }) {
            return vec![];
        }
        match result {
            Ok((data, media_type)) => {
                feed.update(|feed| {
                    feed.uploads.remove(id.as_str());
                });
                self.attach_bytes(feed, id, kind, name, media_type, Bytes::Local(data))
            }
            Err(error) => {
                feed.update(|feed| {
                    if let Some(state) = feed.uploads.get_mut(id.as_str()) {
                        state.status = UploadStatus::Failed(error);
                    }
                });
                vec![]
            }
        }
    }
}
fn failure(code: &str, message: &str) -> ClientError {
    ClientError {
        code: code.into(),
        message: message.into(),
    }
}
fn image_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
fn validate_bytes(
    kind: &AttachmentKind,
    name: &str,
    media: &str,
    data: &[u8],
) -> Result<(), ClientError> {
    let cap = match kind {
        AttachmentKind::Image => ATTACHMENT_IMAGE_MAX,
        AttachmentKind::File => ATTACHMENT_TEXT_MAX,
        _ => {
            return Err(failure(
                "local_type_unsupported",
                "Only images and UTF-8 text files can be uploaded.",
            ));
        }
    };
    if data.is_empty() || data.len() as u64 > cap {
        return Err(failure(
            "local_too_large",
            "Attachment is empty or exceeds its size limit.",
        ));
    }
    if name.is_empty() || name.len() > ATTACHMENT_NAME_MAX || name.chars().any(char::is_control) {
        return Err(failure(
            "local_type_unsupported",
            "Attachment name is invalid or too long.",
        ));
    }
    let supported = match kind {
        AttachmentKind::Image => image_type(data) == Some(media),
        AttachmentKind::File => {
            media == "text/plain" && !data.contains(&0) && std::str::from_utf8(data).is_ok()
        }
        _ => false,
    };
    if supported {
        Ok(())
    } else {
        Err(failure(
            "local_type_unsupported",
            "Use PNG, JPEG, GIF, WebP or a UTF-8 text file.",
        ))
    }
}
#[cfg(not(target_arch = "wasm32"))]
const LOCAL_READ_WORKERS: usize = 2;
#[cfg(not(target_arch = "wasm32"))]
type LocalRead = Box<dyn FnOnce() + Send>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct LocalReads {
    pending: VecDeque<LocalRead>,
    workers: usize,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default)]
struct LocalReadQueue(Arc<Mutex<LocalReads>>);

#[cfg(not(target_arch = "wasm32"))]
impl LocalReadQueue {
    fn push(&self, job: LocalRead) -> std::io::Result<()> {
        // Enqueue and worker retirement share the lock so a job cannot miss its drainer.
        let mut state = self.0.lock().unwrap();
        if state.workers < LOCAL_READ_WORKERS {
            let queue = self.clone();
            match std::thread::Builder::new()
                .name("corral-local-read".into())
                .spawn(move || queue.drain())
            {
                Ok(_) => state.workers += 1,
                Err(error) if state.workers == 0 => return Err(error),
                // A running worker can still take this job.
                Err(_) => {}
            }
        }
        state.pending.push_back(job);
        Ok(())
    }

    fn drain(&self) {
        loop {
            let job = {
                let mut state = self.0.lock().unwrap();
                let Some(job) = state.pending.pop_front() else {
                    state.workers -= 1;
                    return;
                };
                job
            };
            // Never hold the queue lock during disk I/O or completion delivery.
            job();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn enqueue_local_read(request: requests::AttachPath, name: String) -> std::io::Result<()> {
    static QUEUE: OnceLock<LocalReadQueue> = OnceLock::new();
    let queue = QUEUE.get_or_init(LocalReadQueue::default);
    let load = move || ClientRequest::AttachLoaded {
        result: read_local(&request.path, &request.kind),
        upload: request.upload,
        kind: request.kind,
        name,
    };
    #[cfg(not(windows))]
    queue.push(Box::new(move || {
        requests::send(load());
    }))?;
    #[cfg(windows)]
    {
        // The Windows request sender is thread-local to the UI executor.
        let (sender, receiver) = futures_channel::oneshot::channel();
        queue.push(Box::new(move || {
            let _ = sender.send(load());
        }))?;
        spawn(async move {
            if let Ok(loaded) = receiver.await {
                requests::send(loaded);
            }
        });
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn local_name(path: &str) -> String {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment");
    let mut result = String::new();
    for character in name.chars().filter(|c| !c.is_control()) {
        if result.len() + character.len_utf8() > ATTACHMENT_NAME_MAX {
            break;
        }
        result.push(character);
    }
    if result.is_empty() {
        "attachment".into()
    } else {
        result
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn read_local(path: &str, kind: &AttachmentKind) -> Result<(Vec<u8>, String), ClientError> {
    use std::io::Read;
    let cap = match kind {
        AttachmentKind::Image => ATTACHMENT_IMAGE_MAX,
        AttachmentKind::File => ATTACHMENT_TEXT_MAX,
        _ => {
            return Err(failure(
                "local_type_unsupported",
                "Only images and UTF-8 text files can be uploaded.",
            ));
        }
    };
    let path = if let Some(rest) = path.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join(rest))
            .ok_or_else(|| {
                failure(
                    "local_read_failed",
                    "Cannot resolve the client home directory.",
                )
            })?
    } else {
        std::path::PathBuf::from(path)
    };
    #[cfg(unix)]
    let file: std::fs::File = rustix::fs::open(
        &path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| failure("local_read_failed", "Cannot open that local file."))?
    .into();
    #[cfg(not(unix))]
    let file = std::fs::File::open(&path)
        .map_err(|_| failure("local_read_failed", "Cannot open that local file."))?;
    let metadata = file
        .metadata()
        .map_err(|_| failure("local_read_failed", "Cannot inspect that local file."))?;
    if !metadata.is_file() {
        return Err(failure(
            "local_not_regular_file",
            "Choose a regular local file.",
        ));
    }
    if metadata.len() == 0 || metadata.len() > cap {
        return Err(failure(
            "local_too_large",
            "Attachment is empty or exceeds its size limit.",
        ));
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(cap + 1)
        .read_to_end(&mut data)
        .map_err(|_| failure("local_read_failed", "Cannot read that local file."))?;
    let media = match kind {
        AttachmentKind::Image => image_type(&data)
            .ok_or_else(|| failure("local_type_unsupported", "Use PNG, JPEG, GIF or WebP."))?,
        _ => "text/plain",
    };
    validate_bytes(
        kind,
        &local_name(path.to_str().unwrap_or("attachment")),
        media,
        &data,
    )?;
    Ok((data, media.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn local_reads_queue_a_burst_behind_two_workers() {
        use std::{sync::mpsc, time::Duration};

        let queue = LocalReadQueue::default();
        let gate = Arc::new(Mutex::new(()));
        let blocked = gate.lock().unwrap();
        let (started, starts) = mpsc::channel();
        let (finished, finishes) = mpsc::channel();
        let jobs = 64;
        let timeout = Duration::from_secs(5);
        for n in 0..jobs {
            let (gate, started, finished) = (gate.clone(), started.clone(), finished.clone());
            queue
                .push(Box::new(move || {
                    let _ = started.send(n);
                    let _guard = gate.lock();
                    let _ = finished.send(n);
                }))
                .unwrap();
        }
        let mut running: Vec<_> = (0..LOCAL_READ_WORKERS)
            .map(|_| starts.recv_timeout(timeout).unwrap())
            .collect();
        running.sort_unstable();
        assert_eq!(running, (0..LOCAL_READ_WORKERS).collect::<Vec<_>>());
        {
            let state = queue.0.lock().unwrap();
            assert_eq!(state.workers, LOCAL_READ_WORKERS);
            assert_eq!(state.pending.len(), jobs - LOCAL_READ_WORKERS);
        }
        drop(blocked);

        let mut completed: Vec<_> = (0..jobs)
            .map(|_| finishes.recv_timeout(timeout).unwrap())
            .collect();
        completed.sort_unstable();
        assert_eq!(completed, (0..jobs).collect::<Vec<_>>());
    }

    #[test]
    fn text_and_image_boundary_checks_precede_upload() {
        assert!(
            validate_bytes(
                &AttachmentKind::File,
                "a.txt",
                "text/plain",
                &vec![b'x'; ATTACHMENT_TEXT_MAX as usize]
            )
            .is_ok()
        );
        assert!(
            validate_bytes(
                &AttachmentKind::File,
                "a.txt",
                "text/plain",
                &vec![b'x'; ATTACHMENT_TEXT_MAX as usize + 1]
            )
            .is_err()
        );
        assert!(validate_bytes(&AttachmentKind::File, "a.txt", "text/plain", b"x\0y").is_err());
        assert!(
            validate_bytes(
                &AttachmentKind::Image,
                "x.png",
                "image/jpeg",
                b"\x89PNG\r\n\x1a\nx"
            )
            .is_err()
        );
    }
    #[test]
    fn more_than_eight_uploads_queue_and_finish_with_bounded_concurrency() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed {
            capabilities: [ASSISTANT_ATTACH_V1.to_owned()].into(),
            ..Feed::default()
        });
        let mut app = AppState::new();
        app.ready = true;
        let mut outgoing = Vec::new();
        for n in 0..9 {
            outgoing.extend(app.attach_bytes(
                feed,
                format!("upload-{n}"),
                AttachmentKind::File,
                format!("file-{n}.txt"),
                "text/plain".into(),
                Bytes::Local(vec![b'x']),
            ));
        }
        assert_eq!(outgoing.len(), 2);
        while let Some((method, params, tag)) = outgoing.pop() {
            let ready = method == ATTACHMENT_PUT;
            let ack = UploadAck {
                upload: params["upload"].as_str().unwrap().to_owned(),
                received: u64::from(ready),
                bytes: 1,
                ready,
                replayed: false,
            };
            outgoing.extend(
                app.upload_reply(feed, method, tag, params, Ok(value(ack)), None)
                    .unwrap(),
            );
            assert!(outgoing.len() <= 2);
        }
        feed.with_untracked(|feed| {
            assert_eq!(feed.uploads.len(), 9);
            assert!(
                feed.uploads
                    .values()
                    .all(|item| item.status == UploadStatus::Ready)
            );
        });
    }

    #[test]
    fn reconnect_resumes_same_identity_with_two_uploads_and_one_chunk_each() {
        let mut app = AppState::new();
        app.ready = true;
        for id in ["a", "b", "c"] {
            let data = vec![b'x'; ATTACHMENT_CHUNK + 1];
            app.uploads.insert(
                id.into(),
                Upload {
                    begin: AttachmentBegin {
                        upload: id.into(),
                        kind: AttachmentKind::File,
                        name: "log.txt".into(),
                        media_type: "text/plain".into(),
                        bytes: data.len() as u64,
                        sha256: "a".repeat(64),
                    },
                    data: Some(Bytes::Local(data)),
                    received: 0,
                    begun: false,
                    ready: false,
                    failed: false,
                    inflight: None,
                },
            );
        }
        let begins = app.pump_uploads();
        assert_eq!(begins.len(), 2);
        assert!(app.pump_uploads().is_empty());
        let upload = app.uploads.get_mut("a").unwrap();
        upload.inflight = None;
        upload.begun = true;
        upload.received = ATTACHMENT_CHUNK as u64;
        let mut stale_put = app.pump_uploads();
        assert_eq!(stale_put.len(), 1);
        assert_eq!(stale_put[0].0, ATTACHMENT_PUT);
        assert_eq!(stale_put[0].1["offset"], ATTACHMENT_CHUNK);
        assert_eq!(
            STANDARD
                .decode(stale_put[0].1["data"].as_str().unwrap())
                .unwrap(),
            b"x"
        );
        for upload in app.uploads.values_mut() {
            upload.disconnected();
        }
        let resumed = app.pump_uploads();
        assert_eq!(resumed.len(), 2);
        assert_eq!(resumed[0].1, begins[0].1);
        app.filter_outgoing(&mut stale_put);
        assert!(stale_put.is_empty());
        // Suppressed tuples were never registered with Core and leave no deadline behind.
        let mut core = crate::client::core::Core::new();
        for (method, params, tag) in stale_put {
            core.request_tagged(method, params, tag, 0).unwrap();
        }
        assert_eq!(core.expired(6_000), None);
    }
}

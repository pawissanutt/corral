//! Persistent devices and the revocation flags observed by their live connections.
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_protocol::app::SeenCursor;
use corral_protocol::{device::*, rpc::ErrorCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    conn,
    identity::{self, Identity},
    pairing::{PairError, Pairings},
    state::Shared,
};

type Records = BTreeMap<String, DeviceRecord>;
const MAX_DEVICES: usize = 256;

#[path = "devices_push.rs"]
mod push;
#[path = "devices_reach.rs"]
pub(crate) mod reach;
use reach::AppListener;

#[derive(Serialize, Deserialize, Clone)]
struct DeviceRecord {
    name: String,
    platform: DevicePlatform,
    public_key: Option<String>,
    app_version: String,
    enrolled_at: String,
    revoked_at: Option<String>,
    #[serde(default)]
    seen: SeenCursor,
    #[serde(default)]
    last_seen: Option<String>,
    #[serde(default)]
    reach: Option<DeviceReach>,
    #[serde(default)]
    push: push::Registration,
}

impl DeviceRecord {
    fn info(&self, id: &str) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: self.name.clone(),
            platform: self.platform.clone(),
            app_version: self.app_version.clone(),
            enrolled_at: self.enrolled_at.clone(),
            revoked_at: self.revoked_at.clone(),
            last_seen: self.last_seen.clone(),
            reach: self.reach,
        }
    }
}

struct RegisteredSession {
    device: String,
    revoked: Arc<AtomicBool>,
}

struct Registry {
    file: PathBuf,
    records: Mutex<Records>,
}

impl Registry {
    fn open(root: &Path) -> Result<Self> {
        let file = root.join("devices.json");
        let records = match identity::open_private(&file)? {
            Some(input) => serde_json::from_reader(std::io::BufReader::new(input))
                .with_context(|| format!("reading {}", file.display()))?,
            None => Records::new(),
        };
        Ok(Self {
            file,
            records: Mutex::new(records),
        })
    }

    fn active<'a>(records: &'a Records, device: &str) -> Result<&'a DeviceRecord, DeviceError> {
        let record = records
            .get(device)
            .ok_or(DeviceError::NotFound("device_not_found"))?;
        if record.revoked_at.is_some() {
            return Err(DeviceError::Revoked);
        }
        Ok(record)
    }

    // Hold the registry mutex through the atomic replacement. On failure the
    // in-memory view is restored before any connection can see it.
    fn save(&self, records: &mut Records, id: &str, record: DeviceRecord) -> Result<()> {
        let previous = match records.get_mut(id) {
            Some(previous) => Some(std::mem::replace(previous, record)),
            None => records.insert(id.into(), record),
        };
        if let Err(error) = self.persist(records) {
            if let Some(previous) = previous {
                records.insert(id.into(), previous);
            } else {
                records.remove(id);
            }
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self, records: &Records) -> Result<()> {
        let root = self
            .file
            .parent()
            .context("device registry has no parent")?;
        let mut temp = tempfile::NamedTempFile::new_in(root).with_context(|| {
            format!("creating a temporary device registry in {}", root.display())
        })?;
        let mut writer = std::io::BufWriter::new(temp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, records).context("encoding device registry")?;
        writer.write_all(b"\n").context("writing device registry")?;
        writer.flush().context("flushing device registry")?;
        drop(writer);
        temp.as_file()
            .sync_all()
            .context("syncing device registry")?;
        temp.persist(&self.file)
            .with_context(|| format!("replacing {}", self.file.display()))?;
        Ok(())
    }
}

pub(crate) struct Devices {
    registry: Arc<Registry>,
    state_root: PathBuf,
    identity: Mutex<Option<Identity>>,
    listener: Mutex<Option<AppListener>>,
    push: Option<push::Service>,
    pub(crate) pairings: Pairings,
    sessions: Mutex<BTreeMap<u64, RegisteredSession>>,
    next: AtomicU64,
}

pub(crate) struct SessionGuard {
    devices: Arc<Devices>,
    id: u64,
    pub(crate) device: String,
    pub(crate) revoked: Arc<AtomicBool>,
    reach: DeviceReach,
    last_touch: Mutex<Instant>,
}

impl SessionGuard {
    pub(crate) fn touch(&self) -> Result<()> {
        let mut touched = lock(&self.last_touch);
        if touched.elapsed() >= Duration::from_secs(60) {
            match self.devices.touch(&self.device, self.reach) {
                // The connection's revocation flag handles its protocol close.
                Err(DeviceError::Revoked) => {}
                result => result?,
            }
            *touched = Instant::now();
        }
        Ok(())
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session) = lock(&self.devices.sessions).remove(&self.id) {
            debug_assert_eq!(session.device, self.device);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub(crate) fn validate(name: &str, app_version: &str) -> Result<(String, String), DeviceError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(DeviceError::Invalid("invalid_device_name"));
    }
    if app_version.is_empty()
        || app_version.len() > 32
        || !app_version.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(DeviceError::Invalid("invalid_app_version"));
    }
    Ok((name.into(), app_version.into()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

impl Devices {
    pub(crate) fn open(state_root: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_root)
            .with_context(|| format!("creating daemon state directory {}", state_root.display()))?;
        Ok(Self {
            registry: Arc::new(Registry::open(state_root)?),
            state_root: state_root.into(),
            identity: Mutex::new(None),
            listener: Mutex::new(None),
            push: None,
            pairings: Pairings::default(),
            sessions: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(0),
        })
    }

    pub(crate) fn identity(&self) -> Result<Identity, DeviceError> {
        let mut identity = lock(&self.identity);
        if let Some(identity) = *identity {
            return Ok(identity);
        }
        let loaded = identity::load_or_create(&self.state_root)?;
        *identity = Some(loaded);
        Ok(loaded)
    }

    pub(crate) fn store_name(&self) -> String {
        self.state_root.display().to_string()
    }

    fn enrol(
        &self,
        name: &str,
        app_version: &str,
        public: Option<&[u8; 32]>,
    ) -> Result<DeviceInfo, DeviceError> {
        let (name, app_version) = validate(name, app_version)?;
        let mut records = lock(&self.registry.records);
        if records.len() >= MAX_DEVICES {
            return Err(DeviceError::Limit("device_limit"));
        }
        let id = loop {
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes).context("generating device id")?;
            let id = format!("dev-{}", hex(&bytes));
            if !records.contains_key(&id) {
                break id;
            }
        };
        let platform = if public.is_some() {
            DevicePlatform::Web
        } else {
            DevicePlatform::Native
        };
        let record = DeviceRecord {
            name,
            platform,
            public_key: public.map(|key| hex(key)),
            app_version,
            enrolled_at: jiff::Timestamp::now().to_string(),
            revoked_at: None,
            seen: SeenCursor::default(),
            last_seen: None,
            reach: None,
            push: push::Registration::default(),
        };
        let info = record.info(&id);
        self.registry.save(&mut records, &id, record)?;
        Ok(info)
    }

    pub(crate) fn enrol_native(
        &self,
        name: &str,
        app_version: &str,
    ) -> Result<DeviceInfo, DeviceError> {
        self.enrol(name, app_version, None)
    }

    fn register(self: &Arc<Self>, device: String, reach: DeviceReach) -> SessionGuard {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let revoked = Arc::new(AtomicBool::new(false));
        lock(&self.sessions).insert(
            id,
            RegisteredSession {
                device: device.clone(),
                revoked: Arc::clone(&revoked),
            },
        );
        SessionGuard {
            devices: Arc::clone(self),
            id,
            device,
            revoked,
            reach,
            last_touch: Mutex::new(Instant::now()),
        }
    }

    pub(crate) fn attach(
        self: &Arc<Self>,
        device: &str,
    ) -> Result<(DeviceInfo, SessionGuard), DeviceError> {
        // Registry locking serializes authentication/registration against revocation.
        let mut records = lock(&self.registry.records);
        if Registry::active(&records, device)?.platform == DevicePlatform::Web {
            return Err(DeviceError::Conflict("device_is_web"));
        }
        self.touch_record(&mut records, device, DeviceReach::Unix)?;
        let info = Registry::active(&records, device)?.info(device);
        let guard = self.register(device.into(), DeviceReach::Unix);
        Ok((info, guard))
    }

    pub(crate) fn attach_key(
        self: &Arc<Self>,
        public: &[u8; 32],
        reach: DeviceReach,
    ) -> Result<(DeviceInfo, SessionGuard), DeviceError> {
        let mut records = lock(&self.registry.records);
        let public = hex(public);
        let id = records
            .iter()
            .find(|(_, record)| {
                record.platform == DevicePlatform::Web
                    && record.revoked_at.is_none()
                    && record.public_key.as_deref() == Some(public.as_str())
            })
            .map(|(id, _)| id.clone())
            .ok_or(DeviceError::NotFound("device_not_found"))?;
        self.touch_record(&mut records, &id, reach)?;
        let info = Registry::active(&records, &id)?.info(&id);
        let guard = self.register(id, reach);
        Ok((info, guard))
    }

    pub(crate) fn list(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        Ok(lock(&self.registry.records)
            .iter()
            .map(|(id, record)| record.info(id))
            .collect())
    }

    pub(crate) fn revoke(&self, device: &str) -> Result<DeviceInfo, DeviceError> {
        let mut records = lock(&self.registry.records);
        let mut record = records
            .get(device)
            .ok_or(DeviceError::NotFound("device_not_found"))?
            .clone();
        if record.revoked_at.is_none() {
            record.revoked_at = Some(jiff::Timestamp::now().to_string());
            record.push = push::Registration::default();
        } else {
            return Ok(record.info(device));
        }
        let info = record.info(device);
        self.registry.save(&mut records, device, record)?;
        for session in lock(&self.sessions).values() {
            if session.device == device {
                session.revoked.store(true, Ordering::Release);
            }
        }
        Ok(info)
    }

    pub(crate) fn authenticated(&self, device: &str) -> Result<DeviceInfo, DeviceError> {
        let records = lock(&self.registry.records);
        Ok(Registry::active(&records, device)?.info(device))
    }

    pub(crate) fn seen(&self, device: &str) -> Result<SeenCursor, DeviceError> {
        Ok(Registry::active(&lock(&self.registry.records), device)?
            .seen
            .clone())
    }

    pub(crate) fn advance_seen(
        &self,
        device: &str,
        through: u64,
    ) -> Result<SeenCursor, DeviceError> {
        let mut records = lock(&self.registry.records);
        let record = Registry::active(&records, device)?;
        if through <= record.seen.through {
            return Ok(record.seen.clone());
        }
        let mut record = record.clone();
        record.seen = SeenCursor {
            through,
            at: Some(jiff::Timestamp::now().to_string()),
        };
        let seen = record.seen.clone();
        self.registry.save(&mut records, device, record)?;
        Ok(seen)
    }

    pub(crate) fn pair_confirm(&self, pair_id: &str) -> Result<DeviceInfo, DeviceError> {
        let (public, name, app_version) = self.pairings.begin_confirm(pair_id, Instant::now())?;
        let result = self.enrol(&name, &app_version, Some(&public));
        self.pairings
            .finish(pair_id, result.as_ref().cloned().map_err(|_| ()));
        result
    }
}

#[cfg(test)]
#[path = "devices_tests.rs"]
mod tests;

#[derive(Debug)]
pub(crate) enum DeviceError {
    NotFound(&'static str),
    Revoked,
    Expired,
    Conflict(&'static str),
    Limit(&'static str),
    Invalid(&'static str),
    Refused(&'static str),
    Unavailable(String),
    Internal(anyhow::Error),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.wire().1)
    }
}

impl std::error::Error for DeviceError {}

impl From<anyhow::Error> for DeviceError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<PairError> for DeviceError {
    fn from(error: PairError) -> Self {
        match error {
            PairError::NotFound => Self::NotFound("pairing_not_found"),
            PairError::Expired => Self::Expired,
            PairError::Conflict(message) => Self::Conflict(message),
            PairError::Limit => Self::Limit("pairing_limit"),
        }
    }
}

impl DeviceError {
    pub(crate) fn wire(&self) -> (ErrorCode, String) {
        let (code, message) = match self {
            Self::NotFound(message) => (ErrorCode::NotFound, (*message).into()),
            Self::Revoked => (ErrorCode::Revoked, "device_revoked".into()),
            Self::Expired => (ErrorCode::Expired, "pairing_expired".into()),
            Self::Conflict(message) => (ErrorCode::Conflict, (*message).into()),
            Self::Limit(message) => (
                if *message == "pairing_limit" {
                    ErrorCode::Busy
                } else {
                    ErrorCode::Refused
                },
                (*message).into(),
            ),
            Self::Invalid(message) => (ErrorCode::InvalidParams, (*message).into()),
            Self::Refused(message) => (ErrorCode::Refused, (*message).into()),
            Self::Unavailable(message) => (ErrorCode::Unavailable, message.clone()),
            Self::Internal(error) => (
                ErrorCode::Internal,
                format!("{error:#}").replace(['\r', '\n'], " "),
            ),
        };
        (code, message)
    }
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T, DeviceError> {
    serde_json::from_value(value).map_err(|_| DeviceError::Invalid("invalid_params"))
}

pub(crate) fn serve_unix(
    conn: &mut UnixStream,
    shared: &Shared,
    id: Value,
    method: &str,
    input: Value,
    attached: &mut Option<SessionGuard>,
) -> Result<()> {
    let devices = shared.devices();
    let result = (|| -> Result<DeviceInfo, DeviceError> {
        let device = if method == DEVICE_ENROL {
            let p: DeviceEnrol = params(input)?;
            devices.enrol_native(&p.name, &p.app_version)?.id
        } else {
            params::<DeviceAttach>(input)?.device
        };
        let (info, guard) = devices.attach(&device)?;
        *attached = Some(guard);
        Ok(info)
    })();
    match result {
        Ok(result) => {
            shared.publish();
            conn::reply(conn, id, result)?;
        }
        Err(problem) => {
            let (code, message) = problem.wire();
            conn::error(conn, id, code, &message)?;
        }
    }
    Ok(())
}

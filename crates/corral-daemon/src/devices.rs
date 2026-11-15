//! Device registry authority is server-owned and editable per device.
//! Records in `devices` retain authority and a monotonic per-device seen cursor.
//! Callers pass only transport-authenticated ids; queued actions reload these facts.
use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::{Actor, Store, records::RecordArea};
use corral_ops::Ctx;
use corral_protocol::app::{DeviceAuthority, DeviceSetAuthority, SeenCursor};
use corral_protocol::{device::*, rpc::ErrorCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    conn,
    identity::{self, Identity},
    pairing::{PairError, Pairings},
    state::Shared,
};

const AREA: RecordArea<DeviceRecord> = RecordArea::new("devices");
const MAX_DEVICES: usize = 256;

#[derive(Serialize, Deserialize, Clone)]
struct DeviceRecord {
    name: String,
    platform: DevicePlatform,
    public_key: Option<String>,
    app_version: String,
    enrolled_at: String,
    revoked_at: Option<String>,
    #[serde(default)]
    authority: Option<DeviceAuthority>,
    #[serde(default)]
    seen: SeenCursor,
}

impl DeviceRecord {
    fn authority(&self) -> DeviceAuthority {
        self.authority.clone().unwrap_or(DeviceAuthority {
            reply: true,
            act: true,
            assistant: true,
            assistant_act: true,
            admin: true,
        })
    }

    fn info(self, id: String) -> DeviceInfo {
        let authority = self.authority();
        DeviceInfo {
            id,
            name: self.name,
            platform: self.platform,
            app_version: self.app_version,
            enrolled_at: self.enrolled_at,
            revoked_at: self.revoked_at,
            authority: Some(authority),
        }
    }
}

struct AppListener {
    port: u16,
    origin: Option<String>,
}

pub(crate) struct Devices {
    store: Mutex<Store>,
    state_root: Option<PathBuf>,
    store_id: String,
    identity: Mutex<Option<Identity>>,
    listener: Mutex<Option<AppListener>>,
    pub(crate) pairings: Pairings,
    sessions: Mutex<BTreeMap<u64, (String, Arc<AtomicBool>)>>,
    next: AtomicU64,
}

pub(crate) struct SessionGuard {
    devices: Arc<Devices>,
    id: u64,
    pub(crate) device: String,
    pub(crate) revoked: Arc<AtomicBool>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some((device, _)) = lock(&self.devices.sessions).remove(&self.id) {
            debug_assert_eq!(device, self.device);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>, state_root: Option<PathBuf>) -> Result<()> {
    let store =
        Store::open_unobserved(ctx.store.root()).context("opening daemon device registry")?;
    // Persist platform defaults for old enrollments so runtime citation checks use the
    // same durable authority facts as direct app actions (missing facts fail closed).
    for (id, mut record) in AREA.list(&store)? {
        if record.authority.is_none() {
            record.authority = Some(record.authority());
            AREA.save(&store, &id, &record)?;
        }
    }
    *lock(&shared.devices) = Some(Arc::new(Devices {
        store: Mutex::new(store),
        state_root,
        store_id: corral_core::agent::paths::store_id(&ctx.store),
        identity: Mutex::new(None),
        listener: Mutex::new(None),
        pairings: Pairings::default(),
        sessions: Mutex::new(BTreeMap::new()),
        next: AtomicU64::new(0),
    }));
    Ok(())
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
    pub(crate) fn identity(&self) -> Result<Identity, DeviceError> {
        let mut identity = lock(&self.identity);
        if let Some(identity) = *identity {
            return Ok(identity);
        }
        let root = self.state_root.as_ref().ok_or_else(|| {
            DeviceError::Unavailable("app identity needs XDG_STATE_HOME or HOME".into())
        })?;
        let loaded = identity::load_or_create(root, &self.store_id)?;
        *identity = Some(loaded);
        Ok(loaded)
    }

    pub(crate) fn set_listener(&self, port: u16, origin: Option<String>) {
        *lock(&self.listener) = Some(AppListener { port, origin });
    }

    pub(crate) fn store_name(&self) -> String {
        lock(&self.store).dir().display().to_string()
    }

    fn enrol(
        &self,
        name: &str,
        app_version: &str,
        public: Option<&[u8; 32]>,
        via: &str,
    ) -> Result<DeviceInfo, DeviceError> {
        let (name, app_version) = validate(name, app_version)?;
        let base = lock(&self.store);
        let store = base.reattributed();
        store.set_attribution(Actor::Owner, via)?;
        let _guard = store
            .try_lock("devices")?
            .ok_or(DeviceError::Conflict("device_registry_busy"))?;
        if AREA.list(&store)?.len() >= MAX_DEVICES {
            return Err(DeviceError::Limit("device_limit"));
        }
        let id = loop {
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes).context("generating device id")?;
            let id = format!("dev-{}", hex(&bytes));
            if AREA.load(&store, &id)?.is_none() {
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
            authority: Some(DeviceAuthority {
                reply: true,
                act: true,
                assistant: true,
                assistant_act: true,
                admin: true,
            }),
            seen: SeenCursor::default(),
        };
        let (event, platform) = if public.is_some() {
            ("device_paired", "web")
        } else {
            ("device_enrolled", "native")
        };
        store.records_transaction(|| {
            AREA.save(&store, &id, &record)?;
            store.log_event(
                None,
                event,
                json!({"device": id, "name": record.name, "platform": platform}),
            )
        })?;
        Ok(record.info(id))
    }

    pub(crate) fn enrol_native(
        &self,
        name: &str,
        app_version: &str,
        via: &str,
    ) -> Result<DeviceInfo, DeviceError> {
        self.enrol(name, app_version, None, via)
    }

    fn register(self: &Arc<Self>, device: String) -> SessionGuard {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let revoked = Arc::new(AtomicBool::new(false));
        lock(&self.sessions).insert(id, (device.clone(), Arc::clone(&revoked)));
        SessionGuard {
            devices: Arc::clone(self),
            id,
            device,
            revoked,
        }
    }

    pub(crate) fn attach(
        self: &Arc<Self>,
        device: &str,
    ) -> Result<(DeviceInfo, SessionGuard), DeviceError> {
        // Registry locking serializes authentication/registration against revocation.
        let store = lock(&self.store);
        let record = AREA
            .load(&store, device)?
            .ok_or(DeviceError::NotFound("device_not_found"))?;
        if record.revoked_at.is_some() {
            return Err(DeviceError::Revoked);
        }
        if record.platform == DevicePlatform::Web {
            return Err(DeviceError::Conflict("device_is_web"));
        }
        let guard = self.register(device.into());
        Ok((record.info(device.into()), guard))
    }

    pub(crate) fn attach_key(
        self: &Arc<Self>,
        public: &[u8; 32],
    ) -> Result<(DeviceInfo, SessionGuard), DeviceError> {
        let store = lock(&self.store);
        let public = hex(public);
        for (id, record) in AREA.list(&store)? {
            if record.platform == DevicePlatform::Web
                && record.revoked_at.is_none()
                && record.public_key.as_deref() == Some(&public)
            {
                let guard = self.register(id.clone());
                return Ok((record.info(id), guard));
            }
        }
        Err(DeviceError::NotFound("device_not_found"))
    }

    pub(crate) fn list(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        // `info` always populates authority, including legacy enrolled records.
        Ok(AREA
            .list(&lock(&self.store))?
            .into_iter()
            .map(|(id, record)| record.info(id))
            .collect())
    }

    pub(crate) fn revoke(&self, device: &str, via: &str) -> Result<DeviceInfo, DeviceError> {
        let base = lock(&self.store);
        let store = base.reattributed();
        store.set_attribution(Actor::Owner, via)?;
        let _guard = store
            .try_lock("devices")?
            .ok_or(DeviceError::Conflict("device_registry_busy"))?;
        let mut record = AREA
            .load(&store, device)?
            .ok_or(DeviceError::NotFound("device_not_found"))?;
        if record.revoked_at.is_none() {
            record.revoked_at = Some(jiff::Timestamp::now().to_string());
            store.records_transaction(|| {
                AREA.save(&store, device, &record)?;
                store.log_event(
                    None,
                    "device_revoked",
                    json!({"device": device, "name": record.name}),
                )
            })?;
        }
        for (id, revoked) in lock(&self.sessions).values() {
            if id == device {
                revoked.store(true, Ordering::Release);
            }
        }
        Ok(record.info(device.into()))
    }

    /// Reloads durable authority; session-cached facts never authorize a side effect.
    pub(crate) fn authenticated(
        &self,
        device: &str,
    ) -> Result<(DeviceInfo, DeviceAuthority), DeviceError> {
        let record = self.active(&lock(&self.store), device)?;
        let authority = record.authority();
        Ok((record.info(device.into()), authority))
    }

    fn active(&self, store: &Store, device: &str) -> Result<DeviceRecord, DeviceError> {
        let record = AREA
            .load(store, device)?
            .ok_or(DeviceError::NotFound("device_not_found"))?;
        if record.revoked_at.is_some() {
            return Err(DeviceError::Revoked);
        }
        Ok(record)
    }

    pub(crate) fn seen(&self, device: &str) -> Result<SeenCursor, DeviceError> {
        Ok(self.active(&lock(&self.store), device)?.seen)
    }

    pub(crate) fn advance_seen(
        &self,
        device: &str,
        through: u64,
    ) -> Result<SeenCursor, DeviceError> {
        let store = lock(&self.store);
        let _guard = store
            .try_lock("devices")?
            .ok_or(DeviceError::Conflict("device_registry_busy"))?;
        let mut record = self.active(&store, device)?;
        if through > record.seen.through {
            record.seen = SeenCursor {
                through,
                at: Some(corral_core::clock::now().to_string()),
            };
            AREA.save(&store, device, &record)?;
        }
        Ok(record.seen)
    }

    /// Only an active administrator may edit device authority.
    pub(crate) fn set_authority(
        &self,
        admin: &str,
        request: DeviceSetAuthority,
    ) -> Result<DeviceInfo, DeviceError> {
        let base = lock(&self.store);
        let store = base.reattributed();
        let _guard = store
            .try_lock("devices")?
            .ok_or(DeviceError::Conflict("device_registry_busy"))?;
        let caller = self.active(&store, admin)?;
        if !caller.authority().admin {
            return Err(DeviceError::Refused("admin_only"));
        }
        let mut record = self.active(&store, &request.device)?;
        store.set_attribution(Actor::Owner, &format!("app:{admin}"))?;
        record.authority = Some(request.authority);
        store.records_transaction(|| {
            AREA.save(&store, &request.device, &record)?;
            store.log_event(
                None,
                "device_authority_changed",
                json!({
                    "device": request.device, "authority": record.authority,
                    "by_device": admin, "name": caller.name,
                }),
            )
        })?;
        Ok(record.info(request.device))
    }

    pub(crate) fn pair_begin(&self, now: Instant) -> Result<PairOffer, DeviceError> {
        let listener = lock(&self.listener);
        let listener = listener.as_ref().ok_or_else(|| DeviceError::Unavailable("app_listener_off: set [daemon] listen and web_origins in .corral/corral.toml, then restart corrald".into()))?;
        let identity = self.identity()?;
        let mut token = [0; 32];
        let mut pair_id = [0; 8];
        getrandom::fill(&mut token).context("generating pairing token")?;
        getrandom::fill(&mut pair_id).context("generating pairing id")?;
        let code = PairCode {
            daemon_public: identity.public,
            token,
            port: listener.port,
            pair_id,
        };
        let pair_id = code.pair_id_hex();
        self.pairings.begin(pair_id.clone(), token, now)?;
        let PairState::Offered { expires_at } =
            self.pairings.status(&pair_id, now, Duration::ZERO)?
        else {
            return Err(DeviceError::Internal(anyhow::anyhow!(
                "new pairing is not offered"
            )));
        };
        let code = code.encode();
        let url = listener
            .origin
            .as_ref()
            .map(|origin| format!("{origin}/#{PAIR_LINK_FRAGMENT}{code}"));
        Ok(PairOffer {
            pair_id,
            code,
            expires_at,
            url,
        })
    }

    pub(crate) fn pair_confirm(&self, pair_id: &str, via: &str) -> Result<DeviceInfo, DeviceError> {
        let (public, name, app_version) = self.pairings.begin_confirm(pair_id, Instant::now())?;
        let result = self.enrol(&name, &app_version, Some(&public), via);
        self.pairings
            .finish(pair_id, result.as_ref().cloned().map_err(|_| ()));
        result
    }
}

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

pub(crate) struct UnixSession {
    pub(crate) via: &'static str,
    pub(crate) attached: Option<SessionGuard>,
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T, DeviceError> {
    serde_json::from_value(value).map_err(|_| DeviceError::Invalid("invalid_params"))
}
fn value(result: impl Serialize) -> Result<Value, DeviceError> {
    serde_json::to_value(result)
        .context("encoding device reply")
        .map_err(Into::into)
}

pub(crate) fn serve_unix(
    conn: &mut UnixStream,
    shared: &Shared,
    id: Value,
    method: &str,
    input: Value,
    session: &mut UnixSession,
) -> Result<bool> {
    if ![
        DEVICE_ENROL,
        DEVICE_ATTACH,
        DEVICE_LIST,
        DEVICE_REVOKE,
        PAIR_BEGIN,
        PAIR_STATUS,
        PAIR_CONFIRM,
        PAIR_CANCEL,
    ]
    .contains(&method)
    {
        return Ok(false);
    }
    let Some(devices) = shared.devices() else {
        conn::error(conn, id, ErrorCode::Unavailable, "unavailable")?;
        return Ok(true);
    };
    let result = (|| -> Result<Value, DeviceError> {
        if session.via == "app"
            && [DEVICE_REVOKE, PAIR_BEGIN, PAIR_CONFIRM, PAIR_CANCEL].contains(&method)
        {
            let device = session
                .attached
                .as_ref()
                .ok_or(DeviceError::Refused("admin_only"))?;
            let (_, authority) = devices.authenticated(&device.device)?;
            if !authority.admin {
                return Err(DeviceError::Refused("admin_only"));
            }
        }
        let app_via = (session.via == "app")
            .then(|| {
                session
                    .attached
                    .as_ref()
                    .map(|guard| format!("app:{}", guard.device))
            })
            .flatten();
        let via = app_via.as_deref().unwrap_or(session.via);
        match method {
            DEVICE_ENROL => {
                let p: DeviceEnrol = params(input)?;
                let info = devices.enrol_native(&p.name, &p.app_version, via)?;
                let (_, guard) = devices.attach(&info.id)?;
                session.attached = Some(guard);
                value(info)
            }
            DEVICE_ATTACH => {
                let p: DeviceAttach = params(input)?;
                let (info, guard) = devices.attach(&p.device)?;
                session.attached = Some(guard);
                value(info)
            }
            DEVICE_LIST => {
                let _: DeviceList = params(input)?;
                value(devices.list()?)
            }
            DEVICE_REVOKE => {
                let p: DeviceRevoke = params(input)?;
                value(devices.revoke(&p.device, via)?)
            }
            PAIR_BEGIN => {
                let _: PairBegin = params(input)?;
                value(devices.pair_begin(Instant::now())?)
            }
            PAIR_STATUS => {
                let p: PairStatusQuery = params(input)?;
                let state = devices.pairings.status(
                    &p.pair_id,
                    Instant::now(),
                    Duration::from_millis(u64::from(p.wait_ms.min(25_000))),
                )?;
                value(PairStatus {
                    pair_id: p.pair_id,
                    state,
                })
            }
            PAIR_CONFIRM => {
                let p: PairConfirm = params(input)?;
                value(devices.pair_confirm(&p.pair_id, via)?)
            }
            PAIR_CANCEL => {
                let p: PairCancel = params(input)?;
                devices.pairings.cancel(&p.pair_id, Instant::now())?;
                value(())
            }
            _ => Err(DeviceError::Invalid("invalid_device_method")),
        }
    })();
    match result {
        Ok(result) => conn::reply(conn, id, result)?,
        Err(problem) => {
            let (code, message) = problem.wire();
            conn::error(conn, id, code, &message)?;
        }
    }
    Ok(true)
}

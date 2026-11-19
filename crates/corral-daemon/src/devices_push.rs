//! Opt-in, content-free Web Push. Only the push service receives the ECE ciphertext;
//! a browser reconnects through Noise before it can learn any workspace facts.
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use corral_types::{Event, EventKind};
use isahc::config::{Configurable, RedirectPolicy};
use web_push::{
    ContentEncoding, IsahcWebPushClient, SubscriptionInfo, VapidSignatureBuilder, WebPushClient,
    WebPushMessageBuilder,
};

use super::*;

const ATTEMPT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Registration {
    subscription: Option<PushSubscription>,
    kinds: Vec<PushKind>,
    delivered: Option<String>,
    error: Option<String>,
    #[serde(default)]
    sent: u64,
    #[serde(default)]
    attempted: Option<String>,
    #[serde(default)]
    revision: u64,
}

struct Transport {
    pem: Vec<u8>,
    public: String,
    subject: String,
    origins: Vec<String>,
    client: IsahcWebPushClient,
}

impl Transport {
    fn configured(path: &Path, subject: &str, origins: &[String]) -> Result<Self> {
        anyhow::ensure!(
            path.is_absolute(),
            "daemon.push.vapid_private_key must be absolute"
        );
        let metadata = std::fs::symlink_metadata(path).context("reading VAPID key metadata")?;
        anyhow::ensure!(
            metadata.is_file() && metadata.permissions().mode() & 0o077 == 0,
            "VAPID key must be a private regular file (0600)"
        );
        anyhow::ensure!(metadata.len() <= 8192, "VAPID key exceeds 8 KiB");
        anyhow::ensure!(
            subject.starts_with("mailto:") || subject.starts_with("https://"),
            "daemon.push.subject must be a mailto: or https: contact"
        );
        anyhow::ensure!(
            !subject.chars().any(char::is_control) && subject.len() <= 512,
            "invalid daemon.push.subject"
        );
        anyhow::ensure!(
            !origins.is_empty()
                && origins
                    .iter()
                    .all(|origin| endpoint_origin(origin).as_deref() == Some(origin.as_str())),
            "daemon.push.origins must explicitly allow HTTPS push-service origins"
        );
        let pem = std::fs::read(path).context("reading private VAPID key")?;
        let public = URL_SAFE_NO_PAD.encode(
            VapidSignatureBuilder::from_pem_no_sub(pem.as_slice())
                .context("decoding VAPID private key")?
                .get_public_key(),
        );
        let client = isahc::HttpClient::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .redirect_policy(RedirectPolicy::None)
            .build()
            .context("creating bounded Web Push HTTP client")?
            .into();
        Ok(Self {
            pem,
            public,
            subject: subject.into(),
            origins: origins.to_vec(),
            client,
        })
    }

    fn validate(&self, subscription: &PushSubscription) -> Result<(), DeviceError> {
        if subscription.endpoint.len() > 4096
            || subscription.p256dh.len() > 128
            || subscription.auth.len() > 64
        {
            return Err(DeviceError::Invalid("push_subscription_too_large"));
        }
        allow_endpoint(&subscription.endpoint, &self.origins)?;
        let public = URL_SAFE_NO_PAD
            .decode(&subscription.p256dh)
            .map_err(|_| DeviceError::Invalid("invalid_push_public_key"))?;
        let auth = URL_SAFE_NO_PAD
            .decode(&subscription.auth)
            .map_err(|_| DeviceError::Invalid("invalid_push_auth"))?;
        if public.len() != 65 || public.first() != Some(&4) || auth.len() != 16 {
            return Err(DeviceError::Invalid("invalid_push_keys"));
        }
        self.message(subscription)
            .map_err(|_| DeviceError::Invalid("invalid_push_keys"))?;
        Ok(())
    }

    fn message(&self, subscription: &PushSubscription) -> Result<web_push::WebPushMessage> {
        let info = SubscriptionInfo::new(
            &subscription.endpoint,
            &subscription.p256dh,
            &subscription.auth,
        );
        let mut signature = VapidSignatureBuilder::from_pem(self.pem.as_slice(), &info)?;
        signature.add_claim("sub", self.subject.as_str());
        let mut message = WebPushMessageBuilder::new(&info);
        message.set_vapid_signature(signature.build()?);
        message.set_payload(ContentEncoding::Aes128Gcm, b"{}");
        message.set_ttl(300);
        message.set_topic("corral-attention".into());
        Ok(message.build()?)
    }
}

fn endpoint_origin(endpoint: &str) -> Option<String> {
    if endpoint
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '#' || c == '\\')
    {
        return None;
    }
    let uri: tungstenite::http::Uri = endpoint.parse().ok()?;
    let authority = uri.authority()?.as_str();
    if uri.scheme_str() != Some("https") || authority.contains('@') {
        return None;
    }
    Some(format!("https://{authority}"))
}

fn allow_endpoint(endpoint: &str, origins: &[String]) -> Result<(), DeviceError> {
    let origin =
        endpoint_origin(endpoint).ok_or(DeviceError::Invalid("push_endpoint_must_be_https"))?;
    if origins.iter().any(|allowed| allowed == &origin) {
        Ok(())
    } else {
        Err(DeviceError::Refused("push_origin_not_allowlisted"))
    }
}

#[derive(Default)]
struct Signals {
    through: AtomicU64,
    stop: AtomicBool,
}

pub(super) struct Service {
    transport: Arc<Transport>,
    signals: Arc<Signals>,
    wake: SyncSender<()>,
}

impl Service {
    fn start(registry: Arc<Registry>, transport: Transport) -> Result<Self> {
        let transport = Arc::new(transport);
        let signals = Arc::new(Signals::default());
        let (wake, notified) = mpsc::sync_channel(1);
        let worker_transport = Arc::clone(&transport);
        let worker_signals = Arc::clone(&signals);
        std::thread::Builder::new()
            .name("corrald-web-push".into())
            .spawn(move || {
                // A bounded wakeup coalesces batches. Retry failed or rate-limited
                // deliveries on a timer even if no new event arrives.
                while !matches!(
                    notified.recv_timeout(Duration::from_secs(15)),
                    Err(RecvTimeoutError::Disconnected)
                ) && !worker_signals.stop.load(Ordering::Acquire)
                {
                    tick(&registry, &worker_transport, &worker_signals);
                }
            })
            .context("starting Web Push worker")?;
        Ok(Self {
            transport,
            signals,
            wake,
        })
    }

    fn observe(&self, through: u64) {
        if self.signals.through.fetch_max(through, Ordering::AcqRel) < through {
            let _ = self.wake.try_send(());
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.signals.stop.store(true, Ordering::Release);
    }
}

impl Devices {
    pub(crate) fn configure_push(
        &mut self,
        private_key: Option<&Path>,
        subject: &str,
        origins: &[String],
    ) -> Result<()> {
        self.push = private_key
            .map(|path| {
                let transport = Transport::configured(path, subject, origins)?;
                Service::start(Arc::clone(&self.registry), transport)
            })
            .transpose()?;
        Ok(())
    }

    /// The runtime supplies newly committed events. Network I/O runs on the push worker.
    pub(crate) fn observe(&self, events: &[Event]) {
        if let Some(service) = &self.push
            && let Some(through) = attention_seq(events)
        {
            service.observe(through);
        }
    }

    pub(crate) fn push_status(&self, device: &str) -> Result<PushStatus, DeviceError> {
        let records = lock(&self.registry.records);
        Ok(self.push_status_record(Registry::active(&records, device)?))
    }

    fn push_status_record(&self, record: &DeviceRecord) -> PushStatus {
        PushStatus {
            available: self.push.is_some() && record.platform == DevicePlatform::Web,
            vapid_public: self
                .push
                .as_ref()
                .map(|service| service.transport.public.clone()),
            enabled: record.push.subscription.is_some(),
            kinds: record.push.kinds.clone(),
            last_delivered: record.push.delivered.clone(),
            last_error: record.push.error.clone(),
        }
    }

    pub(crate) fn push_subscribe(
        &self,
        device: &str,
        mut request: PushSubscribe,
    ) -> Result<PushStatus, DeviceError> {
        let service = self
            .push
            .as_ref()
            .ok_or_else(|| DeviceError::Unavailable("web_push_not_configured".into()))?;
        service.transport.validate(&request.subscription)?;
        if request.kinds.is_empty() || request.kinds.len() > 5 {
            return Err(DeviceError::Invalid("select_push_kinds"));
        }
        request.kinds.sort();
        request.kinds.dedup();
        let mut records = lock(&self.registry.records);
        let mut record = Registry::active(&records, device)?.clone();
        if record.platform != DevicePlatform::Web {
            return Err(DeviceError::Refused("web_push_requires_browser_device"));
        }
        let revision = record
            .push
            .revision
            .checked_add(1)
            .ok_or(DeviceError::Limit("push_revision_limit"))?;
        record.push = Registration {
            subscription: Some(request.subscription),
            kinds: request.kinds,
            revision,
            sent: service.signals.through.load(Ordering::Acquire),
            ..Registration::default()
        };
        let status = self.push_status_record(&record);
        self.registry.save(&mut records, device, record)?;
        Ok(status)
    }

    pub(crate) fn push_unsubscribe(&self, device: &str) -> Result<PushStatus, DeviceError> {
        let mut records = lock(&self.registry.records);
        let mut record = Registry::active(&records, device)?.clone();
        let revision = record
            .push
            .revision
            .checked_add(1)
            .ok_or(DeviceError::Limit("push_revision_limit"))?;
        record.push = Registration {
            revision,
            ..Registration::default()
        };
        let status = self.push_status_record(&record);
        self.registry.save(&mut records, device, record)?;
        Ok(status)
    }
}

fn attention_seq(events: &[Event]) -> Option<u64> {
    events
        .iter()
        .filter(|event| {
            event.task.is_some() && matches!(event.kind, EventKind::Blocked | EventKind::Ready)
        })
        .map(|event| event.seq)
        .max()
}

fn is_due(record: &DeviceRecord, through: u64, now: jiff::Timestamp) -> bool {
    record.revoked_at.is_none()
        && record.push.subscription.is_some()
        && record.push.kinds.contains(&PushKind::NeedsYou)
        && record.push.sent < through
        && !record
            .push
            .attempted
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .is_some_and(|at| {
                now.as_second().saturating_sub(at.as_second()) < ATTEMPT_INTERVAL.as_secs() as i64
            })
}

fn tick(registry: &Registry, transport: &Transport, signals: &Signals) {
    let through = signals.through.load(Ordering::Acquire);
    if through == 0 {
        return;
    }
    let now = jiff::Timestamp::now();
    let pending: Vec<_> = lock(&registry.records)
        .iter()
        .filter(|(_, record)| is_due(record, through, now))
        .map(|(id, _)| id.clone())
        .collect();
    for id in pending {
        if signals.stop.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = deliver(registry, transport, &id, through) {
            eprintln!("corrald Web Push: {error:#}");
        }
    }
}

fn deliver(registry: &Registry, transport: &Transport, id: &str, through: u64) -> Result<()> {
    let now = jiff::Timestamp::now();
    // Reserve the minute durably before network I/O. Recheck after taking the
    // lock so a concurrent revoke or unsubscribe cannot reserve a delivery.
    let (revision, subscription) = {
        let mut records = lock(&registry.records);
        let Some(record) = records
            .get(id)
            .filter(|record| is_due(record, through, now))
        else {
            return Ok(());
        };
        let Some(subscription) = record.push.subscription.clone() else {
            return Ok(());
        };
        let mut record = record.clone();
        let revision = record.push.revision;
        record.push.attempted = Some(now.to_string());
        registry.save(&mut records, id, record)?;
        (revision, subscription)
    };
    let result = allow_endpoint(&subscription.endpoint, &transport.origins)
        .map_err(anyhow::Error::new)
        .and_then(|()| transport.message(&subscription))
        .and_then(|message| {
            futures_lite::future::block_on(transport.client.send(message)).map_err(Into::into)
        });
    let mut records = lock(&registry.records);
    let Some(record) = records.get(id) else {
        return Ok(());
    };
    if record.revoked_at.is_some() || record.push.revision != revision {
        return Ok(());
    }
    let mut record = record.clone();
    if result.is_ok() {
        record.push.delivered = Some(now.to_string());
        record.push.error = None;
        record.push.sent = through;
    } else {
        // Endpoint URLs contain subscription secrets. Never persist service error bodies.
        record.push.error = Some("push_service_refused_or_unreachable".into());
    }
    registry.save(&mut records, id, record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_endpoint_origins_refuse_credentials_and_insecure_or_ambiguous_urls() {
        for endpoint in [
            "http://push.example/x",
            "https://user@push.example/x",
            "https://push.example/x#secret",
            "https://push.example\\@evil/x",
        ] {
            assert!(endpoint_origin(endpoint).is_none(), "{endpoint}");
        }
        assert_eq!(
            endpoint_origin("https://push.example:8443/send/token?a=b").as_deref(),
            Some("https://push.example:8443")
        );
        let origins = vec!["https://push.example".into()];
        assert!(allow_endpoint("https://push.example/send/token", &origins).is_ok());
        for endpoint in [
            "https://push.example.evil/send",
            "https://evil.example/send",
            "http://push.example/send",
        ] {
            assert!(allow_endpoint(endpoint, &origins).is_err());
        }
    }

    #[test]
    fn blocked_and_ready_are_needs_you_triggers_in_event_order() {
        let event = |seq, kind| Event {
            seq,
            ts: jiff::Timestamp::UNIX_EPOCH,
            task: Some("T-001".into()),
            project: None,
            run: None,
            kind,
            actor: corral_types::Actor::Auto,
            detail: Value::Null,
        };
        assert_eq!(attention_seq(&[event(5, EventKind::Resumed)]), None);
        assert_eq!(attention_seq(&[event(3, EventKind::Blocked)]), Some(3));
        assert_eq!(
            attention_seq(&[event(4, EventKind::Ready), event(5, EventKind::Edited)]),
            Some(4)
        );
    }

    #[test]
    fn subscriptions_default_off_and_unsubscribe_and_revocation_delete_only_the_target() {
        let root = tempfile::tempdir().unwrap();
        let devices = Devices::open(root.path()).unwrap();
        let first = devices.enrol("First", "test", Some(&[1; 32])).unwrap();
        let second = devices.enrol("Second", "test", Some(&[2; 32])).unwrap();
        assert!(!devices.push_status(&first.id).unwrap().enabled);
        assert!(!devices.push_status(&second.id).unwrap().enabled);
        {
            let mut records = lock(&devices.registry.records);
            for id in [&first.id, &second.id] {
                let mut record = records[id].clone();
                record.push.subscription = Some(PushSubscription {
                    endpoint: "https://push.example/secret-token".into(),
                    p256dh: "test-public-key".into(),
                    auth: "test-auth".into(),
                });
                record.push.kinds = vec![PushKind::NeedsYou];
                devices.registry.save(&mut records, id, record).unwrap();
            }
        }
        let status = devices.push_unsubscribe(&first.id).unwrap();
        assert!(!status.enabled);
        assert!(devices.push_status(&second.id).unwrap().enabled);
        devices.revoke(&second.id).unwrap();
        let reopened = Devices::open(root.path()).unwrap();
        let records = lock(&reopened.registry.records);
        for id in [&first.id, &second.id] {
            assert!(records[id].push.subscription.is_none());
        }
    }
}

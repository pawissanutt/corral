//! Opt-in, content-free Web Push. Only the push service receives the ECE ciphertext;
//! a browser reconnects through Noise before it can learn any workspace facts.
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use corral_protocol::{
    DaemonState,
    rows::{AttentionActor, InboxKind, InboxRow, InboxState},
    subscription::Topic,
};
use isahc::config::{Configurable, RedirectPolicy};
use web_push::{
    ContentEncoding, IsahcWebPushClient, SubscriptionInfo, VapidSignatureBuilder, WebPushClient,
    WebPushMessageBuilder,
};

use super::*;

type Attention = BTreeMap<PushKind, BTreeSet<String>>;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Registration {
    subscription: Option<PushSubscription>,
    kinds: Vec<PushKind>,
    delivered: Option<String>,
    error: Option<String>,
    #[serde(default)]
    sent: Attention,
    #[serde(default)]
    attempted: Option<String>,
    #[serde(default)]
    revision: u64,
}

pub(super) struct Service {
    pem: Vec<u8>,
    public: String,
    subject: String,
    origins: Vec<String>,
    client: IsahcWebPushClient,
}

impl Service {
    pub(super) fn configured(config: &corral_core::PushCfg) -> Result<Option<Self>> {
        let Some(path) = &config.vapid_private_key else {
            return Ok(None);
        };
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
            config.subject.starts_with("mailto:") || config.subject.starts_with("https://"),
            "daemon.push.subject must be a mailto: or https: contact"
        );
        anyhow::ensure!(
            !config.subject.chars().any(char::is_control) && config.subject.len() <= 512,
            "invalid daemon.push.subject"
        );
        anyhow::ensure!(
            !config.origins.is_empty()
                && config
                    .origins
                    .iter()
                    .all(|origin| origin.as_str().starts_with("https://")),
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
        Ok(Some(Self {
            pem,
            public,
            subject: config.subject.clone(),
            origins: config
                .origins
                .iter()
                .map(|origin| origin.as_str().to_owned())
                .collect(),
            client,
        }))
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

impl Devices {
    pub(crate) fn push_status(&self, device: &str) -> Result<PushStatus, DeviceError> {
        let record = self.active(&lock(&self.store), device)?;
        Ok(self.push_status_record(&record))
    }

    fn push_status_record(&self, record: &DeviceRecord) -> PushStatus {
        PushStatus {
            available: self.push.is_some() && record.platform == DevicePlatform::Web,
            vapid_public: self.push.as_ref().map(|service| service.public.clone()),
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
        service.validate(&request.subscription)?;
        if request.kinds.is_empty() || request.kinds.len() > 5 {
            return Err(DeviceError::Invalid("select_push_kinds"));
        }
        request.kinds.sort();
        request.kinds.dedup();
        let base = lock(&self.store);
        let store = base.reattributed();
        let _guard = store
            .try_lock("devices")?
            .ok_or(DeviceError::Conflict("device_registry_busy"))?;
        let mut record = self.active(&store, device)?;
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
            ..Registration::default()
        };
        attribute(&store, &format!("app:{device}"))?;
        store.records_transaction(|| {
            AREA.save(&store, device, &record)?;
            store.log_event(
                None,
                "device_push_enabled",
                json!({"device":device,"kinds":record.push.kinds}),
            )
        })?;
        Ok(self.push_status_record(&record))
    }

    pub(crate) fn push_unsubscribe(&self, device: &str) -> Result<PushStatus, DeviceError> {
        let base = lock(&self.store);
        let store = base.reattributed();
        let _guard = store
            .try_lock("devices")?
            .ok_or(DeviceError::Conflict("device_registry_busy"))?;
        let mut record = self.active(&store, device)?;
        let revision = record
            .push
            .revision
            .checked_add(1)
            .ok_or(DeviceError::Limit("push_revision_limit"))?;
        record.push = Registration {
            revision,
            ..Registration::default()
        };
        attribute(&store, &format!("app:{device}"))?;
        store.records_transaction(|| {
            AREA.save(&store, device, &record)?;
            store.log_event(None, "device_push_disabled", json!({"device":device}))
        })?;
        Ok(self.push_status_record(&record))
    }
}

fn attention(rows: &[InboxRow], unhealthy: bool) -> Attention {
    let mut groups = Attention::new();
    for row in rows {
        if row.state != InboxState::Open
            || !row
                .attention
                .as_ref()
                .is_some_and(|a| a.whose_move == AttentionActor::Owner)
        {
            continue;
        }
        let kind = match row.kind {
            InboxKind::PlanAcceptance => PushKind::PlanAwaitingAcceptance,
            InboxKind::Stuck => PushKind::Stuck,
            InboxKind::Escalated => PushKind::Escalated,
            _ => PushKind::NeedsYou,
        };
        groups.entry(kind).or_default().insert(row.id.clone());
    }
    if unhealthy {
        groups
            .entry(PushKind::DaemonUnhealthy)
            .or_default()
            .insert("degraded".into());
    }
    groups
}

pub(super) fn start(devices: &Arc<Devices>, shared: &Arc<Shared>) -> Result<()> {
    if devices.push.is_none() {
        return Ok(());
    }
    let devices = Arc::downgrade(devices);
    let shared = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-web-push".into())
        .spawn(move || {
            let mut watch = None;
            let mut cached = None;
            while !shared.lock().stop {
                let Some(devices) = devices.upgrade() else {
                    break;
                };
                if let Err(error) = tick(&devices, &shared, &mut watch, &mut cached) {
                    eprintln!("corrald Web Push: {error:#}");
                }
                let state = shared.lock();
                drop(
                    shared
                        .changed
                        .wait_timeout_while(state, Duration::from_secs(15), |state| !state.stop)
                        .unwrap_or_else(|error| error.into_inner()),
                );
            }
        })
        .context("starting Web Push worker")?;
    Ok(())
}

fn tick(
    devices: &Devices,
    shared: &Shared,
    watch: &mut Option<crate::views::Watch>,
    cached: &mut Option<(u64, bool, Attention)>,
) -> Result<()> {
    let pending: Vec<_> = AREA
        .list(&lock(&devices.store))?
        .into_iter()
        .filter(|(_, record)| record.revoked_at.is_none() && record.push.subscription.is_some())
        .collect();
    if pending.is_empty() {
        *watch = None;
        *cached = None;
        return Ok(());
    }
    if watch.is_none() {
        *watch = Some(crate::views::watch(
            shared,
            crate::views::Key::new(Topic::Inbox, None)?,
        )?);
    }
    let snapshot = match watch
        .as_ref()
        .map(crate::views::Watch::snapshot)
        .transpose()?
    {
        Some(Some(snapshot)) => snapshot,
        _ => return Ok(()),
    };
    let crate::views::Rows::Inbox(rows) = &snapshot.rows else {
        anyhow::bail!("push watch is not inbox")
    };
    let unhealthy = lock(&shared.health_topic).1.daemon == DaemonState::Degraded;
    let current =
        cached.get_or_insert_with(|| (snapshot.rev, unhealthy, attention(rows, unhealthy)));
    if current.0 != snapshot.rev || current.1 != unhealthy {
        *current = (snapshot.rev, unhealthy, attention(rows, unhealthy));
    }
    for (id, record) in pending {
        if shared.lock().stop {
            break;
        }
        deliver(devices, &id, &record.push, &current.2)?;
    }
    Ok(())
}

fn deliver(devices: &Devices, id: &str, pending: &Registration, current: &Attention) -> Result<()> {
    let selected = current
        .iter()
        .filter(|(kind, _)| pending.kinds.contains(kind));
    if selected.clone().eq(pending.sent.iter()) {
        return Ok(());
    }
    let selected = selected.map(|(kind, ids)| (*kind, ids.clone())).collect();
    let needs_send = has_new(&selected, &pending.sent);
    let now = corral_core::clock::now();
    if needs_send
        && pending
            .attempted
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .is_some_and(|at| now.as_second().saturating_sub(at.as_second()) < 60)
    {
        return Ok(());
    }
    let batch = format!("web-push:{id}:{}:{}", pending.revision, now.as_second());
    // Reserve the minute durably before network I/O. Re-check revocation/subscription
    // under the registry lock; never overwrite a concurrent unsubscribe or authority edit.
    {
        let base = lock(&devices.store);
        let store = base.reattributed();
        let _guard = store
            .try_lock("devices")?
            .context("push registry is busy")?;
        let mut record = devices
            .active(&store, id)
            .map_err(|e| anyhow::anyhow!("{}", e.wire().1))?;
        if record.push.revision != pending.revision || record.push.subscription.is_none() {
            return Ok(());
        }
        if !needs_send {
            record.push.sent = selected;
            AREA.save(&store, id, &record)?;
            return Ok(());
        }
        record.push.attempted = Some(now.to_string());
        store.set_attribution(Actor::System, "web-push")?;
        store.records_transaction(|| {
            AREA.save(&store, id, &record)?;
            store.log_event(
                None,
                "notification_claimed",
                notice_fact(&batch, id, now, None),
            )?;
            Ok(())
        })?;
    }
    let Some(service) = &devices.push else {
        return Ok(());
    };
    let Some(subscription) = &pending.subscription else {
        return Ok(());
    };
    let result = allow_endpoint(&subscription.endpoint, &service.origins)
        .map_err(|error| anyhow::anyhow!("{}", error.wire().1))
        .and_then(|()| service.message(subscription))
        .and_then(|message| {
            futures_lite::future::block_on(service.client.send(message)).map_err(Into::into)
        });
    let base = lock(&devices.store);
    let store = base.reattributed();
    let _guard = store
        .try_lock("devices")?
        .context("push registry is busy")?;
    let Some(mut record) = AREA.load(&store, id)? else {
        return Ok(());
    };
    if record.revoked_at.is_some() || record.push.revision != pending.revision {
        return Ok(());
    }
    store.set_attribution(Actor::System, "web-push")?;
    let (event, outcome) = if result.is_ok() {
        record.push.delivered = Some(now.to_string());
        record.push.error = None;
        record.push.sent = selected;
        (
            "owner_notified",
            corral_core::NoticeOutcome::Notified { at: now },
        )
    } else {
        // Endpoint URLs contain subscription secrets. Never persist HTTP error bodies.
        record.push.error = Some("push_service_refused_or_unreachable".into());
        (
            "owner_notify_failed",
            corral_core::NoticeOutcome::Failed {
                at: now,
                error: "push_service_refused_or_unreachable".into(),
            },
        )
    };
    store.records_transaction(|| {
        AREA.save(&store, id, &record)?;
        store.log_event(None, event, notice_fact(&batch, id, now, Some(outcome)))
    })?;
    Ok(())
}

fn notice_fact(
    batch: &str,
    device: &str,
    at: jiff::Timestamp,
    outcome: Option<corral_core::NoticeOutcome>,
) -> Value {
    // A content-free workspace aggregate has no task/condition coverage. It is a
    // physical send in the shared notice accounting, not a task acknowledgement.
    json!({
        "batch_id": batch, "producer": "web_push", "source": device,
        "claimed_at": at, "claim_ids": [batch], "coverage": [],
        "exemption_basis": null, "outcome": outcome, "device": device,
    })
}

fn has_new(current: &Attention, previous: &Attention) -> bool {
    current
        .iter()
        .any(|(kind, ids)| previous.get(kind).is_none_or(|old| !ids.is_subset(old)))
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
    fn resolving_attention_never_sends_a_new_push_but_a_new_owner_move_does() {
        let old = BTreeMap::from([(
            PushKind::NeedsYou,
            BTreeSet::from(["question-a".into(), "question-b".into()]),
        )]);
        let remaining =
            BTreeMap::from([(PushKind::NeedsYou, BTreeSet::from(["question-b".into()]))]);
        assert!(!has_new(&remaining, &old));
        assert!(!has_new(&Attention::new(), &old));
        assert!(has_new(&old, &remaining));
        let unhealthy = BTreeMap::from([(
            PushKind::DaemonUnhealthy,
            BTreeSet::from(["degraded".into()]),
        )]);
        assert!(has_new(&unhealthy, &remaining));
    }

    #[test]
    fn subscriptions_default_off_and_unsubscribe_and_revocation_delete_only_the_target() {
        let fixture = crate::tests::Fixture::new();
        let shared = Arc::new(Shared::new());
        super::super::start(&fixture.ctx, &shared, None).unwrap();
        let devices = shared.devices().unwrap();
        let first = devices
            .enrol("First", "test", Some(&[1; 32]), "test")
            .unwrap();
        let second = devices
            .enrol("Second", "test", Some(&[2; 32]), "test")
            .unwrap();
        assert!(!devices.push_status(&first.id).unwrap().enabled);
        assert!(!devices.push_status(&second.id).unwrap().enabled);
        {
            let store = lock(&devices.store);
            for id in [&first.id, &second.id] {
                let mut record = AREA.load(&store, id).unwrap().unwrap();
                record.push.subscription = Some(PushSubscription {
                    endpoint: "https://push.example/secret-token".into(),
                    p256dh: "test-public-key".into(),
                    auth: "test-auth".into(),
                });
                record.push.kinds = vec![PushKind::NeedsYou];
                AREA.save(&store, id, &record).unwrap();
            }
        }
        let status = devices.push_unsubscribe(&first.id).unwrap();
        assert!(!status.enabled);
        assert!(devices.push_status(&second.id).unwrap().enabled);
        devices.revoke(&second.id, "test").unwrap();
        let store = lock(&devices.store);
        for id in [&first.id, &second.id] {
            assert!(
                AREA.load(&store, id)
                    .unwrap()
                    .unwrap()
                    .push
                    .subscription
                    .is_none()
            );
        }
    }
}

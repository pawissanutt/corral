//! Reach comes from the authenticated carrier, never client-supplied metadata.
use std::net::{IpAddr, SocketAddr};

use corral_protocol::rows::ListenerInfo;

use super::*;

pub(super) struct AppListener {
    address: SocketAddr,
    origins: Vec<String>,
    origin: Option<String>,
    endpoint: Option<String>,
    reach: DeviceReach,
}

pub(crate) fn classify(ip: IpAddr) -> DeviceReach {
    if ip.is_loopback() {
        DeviceReach::Loopback
    } else if match ip {
        IpAddr::V4(ip) => u32::from(ip) & 0xffc0_0000 == 0x6440_0000,
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    } {
        DeviceReach::Tailscale
    } else {
        DeviceReach::Lan
    }
}

pub(crate) fn hostname() -> Option<String> {
    rustix::system::uname()
        .nodename()
        .to_str()
        .ok()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

impl Devices {
    pub(crate) fn set_listener(
        &self,
        addr: SocketAddr,
        origins: &[String],
        web_endpoint: Option<&str>,
    ) {
        let endpoint = web_endpoint
            .map(str::to_owned)
            .or_else(|| addr.ip().is_loopback().then(|| format!("ws://{addr}")))
            .filter(|url| addr.ip().is_loopback() || url.starts_with("wss://"));
        let origin = if let Some(authority) = endpoint
            .as_deref()
            .and_then(|url| url.strip_prefix("wss://"))
        {
            let matching = format!("https://{authority}");
            origins
                .iter()
                .find(|origin| **origin == matching)
                .or_else(|| origins.iter().find(|origin| origin.starts_with("https://")))
                .cloned()
        } else {
            origins.first().cloned()
        };
        *lock(&self.listener) = Some(AppListener {
            address: addr,
            origins: origins.to_vec(),
            origin,
            endpoint,
            reach: classify(addr.ip()),
        });
    }

    pub(crate) fn listener(&self) -> Option<ListenerInfo> {
        lock(&self.listener).as_ref().map(|listener| ListenerInfo {
            port: listener.address.port(),
            origins: listener.origins.clone(),
            address: Some(listener.address.to_string()),
        })
    }

    pub(crate) fn touch(&self, device: &str, reach: DeviceReach) -> Result<(), DeviceError> {
        self.touch_record(&mut lock(&self.registry.records), device, reach)
    }

    pub(super) fn touch_record(
        &self,
        records: &mut Records,
        device: &str,
        reach: DeviceReach,
    ) -> Result<(), DeviceError> {
        let record = Registry::active(records, device)?;
        let now = jiff::Timestamp::now();
        let recent = record
            .last_seen
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .is_some_and(|at| now.as_second().saturating_sub(at.as_second()) < 60);
        if record.reach == Some(reach) && recent {
            return Ok(());
        }
        let mut record = record.clone();
        record.last_seen = Some(now.to_string());
        record.reach = Some(reach);
        self.registry.save(records, device, record)?;
        Ok(())
    }

    pub(crate) fn pair_begin(&self, now: Instant) -> Result<PairOffer, DeviceError> {
        let listener = lock(&self.listener);
        let listener = listener.as_ref().ok_or_else(|| {
            DeviceError::Unavailable(
                "app_listener_off: set [daemon] listen and web_origins, then restart corrald"
                    .into(),
            )
        })?;
        let endpoint = listener.endpoint.clone().ok_or_else(|| DeviceError::Unavailable(
            "remote_browser_needs_tls: configure daemon.web_endpoint = wss://host and serve the app over HTTPS".into(),
        ))?;
        let identity = self.identity()?;
        let mut token = [0; 32];
        let mut pair_id = [0; 8];
        getrandom::fill(&mut token).context("generating pairing token")?;
        getrandom::fill(&mut pair_id).context("generating pairing id")?;
        let port = listener.address.port();
        let legacy = endpoint
            .strip_prefix("ws://127.0.0.1:")
            .is_some_and(|value| value.parse::<u16>().ok() == Some(port));
        let code = PairCode {
            daemon_public: identity.public,
            token,
            port,
            pair_id,
            endpoint: (!legacy).then(|| endpoint.clone()),
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
            reach: vec![ReachHint {
                reach: listener.reach,
                endpoint,
            }],
        })
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn authenticated_activity_persists_last_seen_and_each_guard_keeps_its_own_reach() {
        let root = tempfile::tempdir().unwrap();
        let devices = Arc::new(Devices::open(root.path()).unwrap());
        let native = devices.enrol_native("Native", "test").unwrap();
        assert!(native.last_seen.is_none());
        let (native, _) = devices.attach(&native.id).unwrap();
        assert!(
            native
                .last_seen
                .as_ref()
                .unwrap()
                .parse::<jiff::Timestamp>()
                .is_ok()
        );
        assert_eq!(native.reach, Some(DeviceReach::Unix));
        let web = devices.enrol("Web", "test", Some(&[7; 32])).unwrap();
        assert!(web.reach.is_none());
        assert!(web.last_seen.is_none());
        let (_, first) = devices
            .attach_key(&[7; 32], DeviceReach::Tailscale)
            .unwrap();
        let (web, second) = devices.attach_key(&[7; 32], DeviceReach::Lan).unwrap();
        assert_eq!(first.reach, DeviceReach::Tailscale);
        assert_eq!(second.reach, DeviceReach::Lan);
        let saved = devices.authenticated(&web.id).unwrap();
        assert_eq!(saved.reach, Some(DeviceReach::Lan));
        assert_eq!(saved.last_seen, web.last_seen);
    }

    #[test]
    fn remote_pairing_requires_tls_and_the_qr_link_never_points_at_the_scanners_loopback() {
        let root = tempfile::tempdir().unwrap();
        let devices = Devices::open(root.path()).unwrap();
        let origins = vec![
            "http://127.0.0.1:8790".into(),
            "https://corral.example".into(),
        ];
        devices.set_listener("127.0.0.1:7841".parse().unwrap(), &origins, None);
        let local = devices.pair_begin(Instant::now()).unwrap();
        assert!(PairCode::decode(&local.code).unwrap().endpoint.is_none());
        devices.set_listener("100.64.0.2:7841".parse().unwrap(), &origins, None);
        assert!(matches!(
            devices.pair_begin(Instant::now()),
            Err(DeviceError::Unavailable(_))
        ));
        devices.set_listener(
            "100.64.0.2:7841".parse().unwrap(),
            &origins,
            Some("wss://workstation.tailnet.ts.net"),
        );
        let remote = devices.pair_begin(Instant::now()).unwrap();
        assert_eq!(remote.reach[0].reach, DeviceReach::Tailscale);
        assert_eq!(
            PairCode::decode(&remote.code).unwrap().endpoint.as_deref(),
            Some("wss://workstation.tailnet.ts.net")
        );
        assert!(
            remote
                .url
                .unwrap()
                .starts_with("https://corral.example/#corral-pair=")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_tracks_the_actual_interface_not_the_build_target() {
        for (ip, reach) in [
            ("127.0.0.1", DeviceReach::Loopback),
            ("::1", DeviceReach::Loopback),
            ("192.168.1.2", DeviceReach::Lan),
            ("100.64.0.2", DeviceReach::Tailscale),
            ("100.128.0.2", DeviceReach::Lan),
            ("fd7a:115c:a1e0::2", DeviceReach::Tailscale),
        ] {
            assert_eq!(classify(ip.parse().unwrap()), reach);
        }
    }
}

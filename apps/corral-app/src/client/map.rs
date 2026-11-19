pub(super) fn timestamp(text: Option<&str>) -> Option<u64> {
    text?
        .parse::<jiff::Timestamp>()
        .ok()?
        .as_second()
        .try_into()
        .ok()
}

pub(super) fn device(
    info: corral_protocol::device::DeviceInfo,
) -> crate::model::devices::DeviceRow {
    use corral_protocol::device::DevicePlatform;
    crate::model::devices::DeviceRow {
        id: info.id,
        name: info.name,
        platform: match info.platform {
            DevicePlatform::Native => "native",
            DevicePlatform::Web => "web",
            DevicePlatform::Other => "other",
        },
        enrolled: info.enrolled_at.chars().take(10).collect(),
        revoked: info.revoked_at.map(|at| at.chars().take(10).collect()),
        last_seen: info.last_seen,
        reach: info.reach.map(|reach| reach_label(reach).into()),
    }
}

fn reach_label(reach: corral_protocol::device::DeviceReach) -> &'static str {
    use corral_protocol::device::DeviceReach;
    match reach {
        DeviceReach::Unix => "unix socket",
        DeviceReach::Loopback => "loopback",
        DeviceReach::Lan => "LAN",
        DeviceReach::Tailscale => "Tailscale",
        DeviceReach::Other => "unknown",
    }
}

pub(super) fn reach(hints: Vec<corral_protocol::device::ReachHint>) -> Vec<(String, String)> {
    hints
        .into_iter()
        .map(|hint| (reach_label(hint.reach).into(), hint.endpoint))
        .collect()
}

pub(super) fn pairing(
    status: corral_protocol::device::PairStatus,
    previous: &mut crate::model::devices::Pairing,
) -> crate::model::devices::Pairing {
    use crate::model::devices::Pairing;
    use corral_protocol::device::PairState;
    match status.state {
        PairState::Offered { expires_at } => {
            let (code, url, reach) = match previous {
                Pairing::Offered {
                    pair_id,
                    code,
                    url,
                    reach,
                    ..
                } if *pair_id == status.pair_id => {
                    (std::mem::take(code), url.take(), std::mem::take(reach))
                }
                _ => (String::new(), None, vec![]),
            };
            Pairing::Offered {
                pair_id: status.pair_id,
                code,
                url,
                expires_at: timestamp(Some(&expires_at)).unwrap_or(0),
                reach,
            }
        }
        PairState::Claimed {
            name,
            app_version,
            sas,
            expires_at,
        } => Pairing::Claimed {
            pair_id: status.pair_id,
            name,
            app_version,
            sas,
            expires_at: timestamp(Some(&expires_at)).unwrap_or(0),
        },
        PairState::Paired { device } => Pairing::Ended {
            line: format!("Paired {}.", device.name),
        },
        PairState::Ended { reason } => Pairing::ended(&reason),
        PairState::Other => Pairing::Ended {
            line: "corrald returned an unsupported pairing state.".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_activity_and_pairing_reach_survive_wire_mapping() {
        use corral_protocol::device::{DeviceInfo, DeviceReach, ReachHint};
        let info: DeviceInfo = serde_json::from_value(serde_json::json!({
            "id": "device-a",
            "name": "Browser",
            "platform": "web",
            "app_version": "test",
            "enrolled_at": "2026-11-01T00:00:00Z",
            "revoked_at": null,
            "last_seen": "2026-11-15T09:10:00Z",
            "reach": "tailscale"
        }))
        .unwrap();
        let row = device(info);
        assert_eq!(row.last_seen.as_deref(), Some("2026-11-15T09:10:00Z"));
        assert_eq!(row.reach.as_deref(), Some("Tailscale"));
        let hints = reach(vec![
            ReachHint {
                reach: DeviceReach::Unix,
                endpoint: "/tmp/corral.sock".into(),
            },
            ReachHint {
                reach: DeviceReach::Loopback,
                endpoint: "ws://127.0.0.1:8790".into(),
            },
            ReachHint {
                reach: DeviceReach::Lan,
                endpoint: "wss://corral.example".into(),
            },
        ]);
        assert_eq!(
            hints
                .iter()
                .map(|(label, endpoint)| (label.as_str(), endpoint.as_str()))
                .collect::<Vec<_>>(),
            [
                ("unix socket", "/tmp/corral.sock"),
                ("loopback", "ws://127.0.0.1:8790"),
                ("LAN", "wss://corral.example"),
            ],
        );
    }

    #[test]
    fn offered_status_retains_link_and_code_only_for_the_same_pairing() {
        use crate::model::daemon::Feed;
        use crate::model::devices::{Pairing, PairingPanel, live_devices};
        use corral_protocol::device::{PairState, PairStatus};

        let mut previous = Pairing::Offered {
            pair_id: "pair-a".into(),
            code: "draft-code".into(),
            url: Some("http://127.0.0.1:8790/#corral-pair=draft-code".into()),
            expires_at: 1,
            reach: vec![("LAN".into(), "wss://corral.example".into())],
        };
        let status = |pair_id: &str| PairStatus {
            pair_id: pair_id.into(),
            state: PairState::Offered {
                expires_at: "2030-01-01T00:00:00Z".into(),
            },
        };
        for _ in 0..2 {
            previous = pairing(status("pair-a"), &mut previous);
            let feed = Feed {
                pairing: previous.clone(),
                ..Feed::default()
            };
            assert!(matches!(
                &previous,
                Pairing::Offered { reach, .. }
                    if reach == &vec![("LAN".into(), "wss://corral.example".into())]
            ));
            assert!(matches!(
                live_devices(&feed).panel,
                PairingPanel::Offered { code, url: Some(url), .. }
                    if code == "draft-code" && url.ends_with("#corral-pair=draft-code")
            ));
        }
        assert!(matches!(
            pairing(status("pair-b"), &mut previous),
            Pairing::Offered { code, url: None, reach, .. } if code.is_empty() && reach.is_empty()
        ));
    }
}

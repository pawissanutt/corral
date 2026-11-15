use crate::model::board::Mark;
use crate::model::daemon::{
    Agent, DaemonHealth, DaemonLevel, Group, PmLevel, Runtime, RuntimeLevel, Step, Task,
};
use corral_protocol::board::{BoardGroup, BoardRow, CostBasis, RailGlyph};
use corral_protocol::{DaemonState, Health, PmState, RuntimeState};

pub(super) fn timestamp(text: Option<&str>) -> Option<u64> {
    text?
        .parse::<jiff::Timestamp>()
        .ok()?
        .as_second()
        .try_into()
        .ok()
}

pub(super) fn task(row: BoardRow) -> Task {
    Task {
        id: row.id.into(),
        title: row.title,
        project: row.project,
        plan: row.plan,
        goal: row.goal,
        group: match row.group {
            BoardGroup::NeedsYou => Group::NeedsYou,
            BoardGroup::Verifying => Group::Verifying,
            BoardGroup::Producing => Group::Producing,
            BoardGroup::Queued => Group::Queued,
            BoardGroup::Held => Group::Held,
            BoardGroup::Parked => Group::Parked,
            BoardGroup::Other => Group::Other,
        },
        rail: row.rail.map(|step| Step {
            mark: match step.glyph {
                RailGlyph::Pass => Mark::Done,
                RailGlyph::Active => Mark::Active,
                RailGlyph::Fail => Mark::Fail,
                RailGlyph::Stale => Mark::Stale,
                RailGlyph::Pending => Mark::Next,
                RailGlyph::Unavailable | RailGlyph::Other => Mark::Na,
            },
            sha7: step.sha7,
            at: timestamp(step.at.as_deref()),
        }),
        now: row.now.text,
        now_code: row.now.code,
        since: timestamp(row.now.since.as_deref()),
        age_since: timestamp(row.age_since.as_deref()),
        held_reason: row.held_reason,
        questions: row.questions,
        attention: row.attention,
        agent: row.agent.map(|agent| Agent {
            session: agent.session,
            role: agent.role,
            profile: agent.profile,
            runtime: agent.runtime,
            machine: agent.machine,
        }),
        cost: row.cost_usd,
        cost_estimated: row.cost_basis != Some(CostBasis::Reported),
        rev: row.rev,
    }
}

pub(super) fn health(health: Health) -> DaemonHealth {
    DaemonHealth {
        level: match health.daemon {
            DaemonState::Healthy => DaemonLevel::Healthy,
            DaemonState::Degraded => DaemonLevel::Degraded,
            DaemonState::Down => DaemonLevel::Down,
            DaemonState::Other => DaemonLevel::Unknown,
        },
        reasons: health.reasons,
        runtimes: health
            .runtimes
            .into_iter()
            .map(|runtime| Runtime {
                kind: runtime.kind,
                level: match runtime.state {
                    RuntimeState::Ok => RuntimeLevel::Ok,
                    RuntimeState::Unreachable => RuntimeLevel::Unreachable,
                    RuntimeState::Other => RuntimeLevel::Unknown,
                },
            })
            .collect(),
        pm: match health.pm {
            PmState::Live => PmLevel::Live,
            PmState::Relaunching => PmLevel::Relaunching,
            PmState::Absent => PmLevel::Absent,
            PmState::Other => PmLevel::Unknown,
        },
        pass_seq: health.loop_facts.pass_seq,
        last_pass: timestamp(health.loop_facts.last_pass_at.as_deref()),
        last_pass_ms: health.loop_facts.last_pass_ms,
        version: health.version,
    }
}

pub(super) fn device(
    info: corral_protocol::device::DeviceInfo,
) -> crate::model::devices::DeviceRow {
    use corral_protocol::device::DevicePlatform;
    crate::model::devices::DeviceRow {
        id: info.id,
        name: info.name,
        authority: info.authority,
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
    use corral_protocol::board::{BoardNow, RailStep};

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
            "authority": null,
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

    #[test]
    fn estimated_row_maps_rfc3339_seconds_and_cost_flag() {
        let row = BoardRow {
            id: "T-9".into(),
            title: "Live row".into(),
            project: None,
            plan: None,
            goal: None,
            group: BoardGroup::Verifying,
            rail: std::array::from_fn(|_| RailStep {
                glyph: RailGlyph::Unavailable,
                sha7: None,
                at: Some("2024-01-01T01:00:00+01:00".into()),
            }),
            now: BoardNow {
                code: "queued".into(),
                text: "queued".into(),
                since: Some("2024-01-01T00:00:00Z".into()),
            },
            age_since: Some("2023-12-01T00:00:00Z".into()),
            held_reason: None,
            questions: 0,
            attention: vec![],
            agent: None,
            cost_usd: Some(1.25),
            cost_basis: Some(CostBasis::Estimated),
            last_event: None,
            rev: 1,
        };
        let mapped = task(row);
        assert_eq!(mapped.since, Some(1_704_067_200));
        assert_eq!(mapped.age_since, timestamp(Some("2023-12-01T00:00:00Z")));
        assert_eq!(mapped.rail[0].at, mapped.since);
        assert_eq!(mapped.rail[0].mark, Mark::Na);
        assert_eq!(mapped.cost, Some(1.25));
        assert!(mapped.cost_estimated);
    }
}

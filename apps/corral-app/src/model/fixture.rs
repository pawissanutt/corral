//! Shell fixtures, with connection health independent of the fixture transport switch.

use crate::model::{
    Conn, DataMode, DataState, Destination, Env, Form, Health, Segment, Snapshot, Tone, Workspace,
    inbox,
};

/// Creates the shell snapshot for this transport, connection and device. App state replaces the
/// initial Inbox count with the current Inbox reducer's count.
pub fn snapshot(state: DataState, conn: Conn, form: Form) -> Snapshot {
    match state {
        DataState::Fixture => Snapshot::new(
            state,
            health(conn, form),
            Some(Workspace {
                name: "corral",
                path: "~/miti-workspace/corral",
                autopilot: "Autopilot · independent work continues",
                device: if form == Form::Phone {
                    "pawis-iphone"
                } else {
                    "pawis-desktop"
                },
                via: if form == Form::Phone {
                    "via=app:pawis-iphone"
                } else {
                    "via=app:pawis-desktop"
                },
            }),
            vec![
                (
                    Destination::Inbox,
                    inbox::needs_you_count(
                        &inbox::State::default(),
                        Env {
                            conn,
                            mode: DataMode::Live,
                            form,
                        },
                    ),
                ),
                (Destination::Changes, 12),
                (Destination::Plans, 6),
                (Destination::Agents, 7),
                (Destination::Review, 3),
            ],
        ),
        DataState::Disconnected => Snapshot::new(
            state,
            Health {
                segments: vec![Segment {
                    name: "corrald",
                    value: "not connected",
                    tone: Tone::Fail,
                }],
                reach: None,
                e2e: false,
                attention: None,
                clock: None,
                source: None,
            },
            None,
            Vec::new(),
        ),
    }
}

/// The four HEALTH segments and the device's reach path. Offline phone health omits the
/// daemon's redundant snapshot suffix because the reach line already says when it last synced.
pub fn health(conn: Conn, form: Form) -> Health {
    let (daemon, runtime, pm, daemon_tone, runtime_tone, pm_tone) = match conn {
        Conn::Healthy | Conn::Relay => ("healthy", "ok", "live", Tone::Ok, Tone::Ok, Tone::Ok),
        Conn::Degraded => (
            "degraded · sync stalled 4m",
            "ok",
            "relaunching",
            Tone::Warn,
            Tone::Ok,
            Tone::Warn,
        ),
        Conn::Down => (
            "down",
            "unreachable",
            "absent",
            Tone::Fail,
            Tone::Fail,
            Tone::Fail,
        ),
        Conn::Offline => (
            if form == Form::Phone {
                "healthy"
            } else {
                "healthy · as of 22:08"
            },
            "ok",
            "live",
            Tone::Warn,
            Tone::Warn,
            Tone::Warn,
        ),
    };
    Health {
        segments: vec![
            Segment {
                name: "corrald",
                value: daemon,
                tone: daemon_tone,
            },
            Segment {
                name: "herdr-core",
                value: runtime,
                tone: runtime_tone,
            },
            Segment {
                name: "herdr-tui",
                value: runtime,
                tone: runtime_tone,
            },
            Segment {
                name: "PM",
                value: pm,
                tone: pm_tone,
            },
        ],
        reach: Some(match conn {
            Conn::Relay => "relay · e2e",
            Conn::Offline => "offline · last sync 22:08",
            _ if form == Form::Phone => "direct · tailscale",
            _ => "direct · loopback",
        }),
        e2e: conn == Conn::Relay,
        attention: Some("12 since 18:40"),
        clock: Some("22:12"),
        source: Some("fixture · snapshot 2026-11-07 22:12"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_has_no_counts_or_healthy_segments() {
        for conn in Conn::ALL {
            let snapshot = snapshot(DataState::Disconnected, conn, Form::Desktop);
            for destination in Destination::ALL {
                assert_eq!(snapshot.count(destination), None);
            }
            assert!(
                snapshot
                    .health
                    .segments
                    .iter()
                    .all(|segment| segment.tone != Tone::Ok)
            );
        }
    }
}

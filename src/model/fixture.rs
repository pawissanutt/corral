use crate::model::{DataState, Destination, Health, Segment, Snapshot, Tone, Workspace};

/// Creates the shell snapshot for the selected data state.
pub fn snapshot(state: DataState) -> Snapshot {
    match state {
        DataState::Fixture => Snapshot::new(
            state,
            Health {
                segments: vec![
                    Segment {
                        name: "corrald",
                        value: "healthy",
                        tone: Tone::Ok,
                    },
                    Segment {
                        name: "herdr-core",
                        value: "ok",
                        tone: Tone::Ok,
                    },
                    Segment {
                        name: "herdr-tui",
                        value: "ok",
                        tone: Tone::Ok,
                    },
                    Segment {
                        name: "PM",
                        value: "live",
                        tone: Tone::Ok,
                    },
                ],
                reach: Some("direct · loopback"),
                attention: Some("12 since 18:40"),
                clock: Some("22:12"),
                source: Some("fixture · snapshot 2026-11-07 22:12"),
            },
            Some(Workspace {
                name: "corral",
                path: "~/miti-workspace/corral",
                autopilot: "Autopilot · independent work continues",
                device: "pawis-desktop",
                via: "via=app:pawis-desktop",
            }),
            vec![
                (Destination::Inbox, 5),
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
                attention: None,
                clock: None,
                source: None,
            },
            None,
            Vec::new(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_has_no_counts_or_healthy_segments() {
        let snapshot = snapshot(DataState::Disconnected);
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

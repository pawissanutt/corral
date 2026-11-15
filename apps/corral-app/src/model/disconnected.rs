//! Missing daemon data stays missing; sample records are owned only by the explicit preview.

use crate::model::{DataState, Health, Segment, Snapshot, Tone};

pub fn snapshot() -> Snapshot {
    Snapshot::new(
        DataState::Disconnected,
        Health {
            segments: vec![Segment {
                name: "corrald".into(),
                value: "not connected".into(),
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Destination;

    #[test]
    fn disconnected_has_no_counts_or_healthy_segments() {
        let snapshot = snapshot();
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

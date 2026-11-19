//! Canonical attribution is retained on the wire; owner copy resolves enrolled names.
use crate::model::daemon::Feed;

/// A human source name, never a raw enrolled-device identifier.
pub fn via_name(feed: &Feed, device: Option<&str>, via: &str) -> String {
    let id = device.or_else(|| via.strip_prefix("app:"));
    if let Some(id) = id {
        let session = feed
            .session
            .as_ref()
            .filter(|session| session.device.id == id);
        let name = session
            .map(|session| session.device.name.as_str())
            .or_else(|| {
                feed.devices
                    .iter()
                    .find(|row| row.id == id)
                    .map(|row| row.name.as_str())
            })
            .or_else(|| match &feed.device {
                crate::model::devices::DeviceLink::Enrolled { id: own, name } if own == id => {
                    Some(name.as_str())
                }
                _ => None,
            });
        return name
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "another device".into());
    }
    match via {
        "pm-pane" | "pm" => "the PM pane".into(),
        "cli" | "terminal" | "tui" => "the terminal".into(),
        "app" => "the app".into(),
        _ => "a recorded session".into(),
    }
}

/// Human actor and source for stamps, threads and action outcomes.
pub fn provenance(feed: &Feed, by: &str, device: Option<&str>, via: &str) -> String {
    let actor = match by {
        "owner" | "you" => "you",
        "pm" => "the PM",
        "worker" => "the worker",
        "assistant" => "the assistant",
        _ => "the recorded actor",
    };
    format!("{actor} on {}", via_name(feed, device, via))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::devices::DeviceRow;

    #[test]
    fn provenance_resolves_device_names_without_leaking_unknown_ids() {
        let mut feed = Feed::default();
        feed.devices.push(DeviceRow {
            id: "secret-device-id".into(),
            name: "Owner phone".into(),
            platform: "web",
            enrolled: String::new(),
            revoked: None,
            last_seen: None,
            reach: None,
        });
        assert_eq!(via_name(&feed, None, "app:secret-device-id"), "Owner phone");
        assert!(
            !provenance(&feed, "owner", None, "app:unknown-device-id")
                .contains("unknown-device-id")
        );
    }
}

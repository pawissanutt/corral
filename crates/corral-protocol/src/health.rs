//! Health snapshots are observations, not proof that any requested action completed.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Healthy,
    Degraded,
    Down,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Health {
    pub daemon: DaemonState,
    pub runner_reachable: bool,
    /// RFC 3339 timestamp, absent until the first sync completes.
    pub last_sync_at: Option<String>,
    pub last_sync_error: Option<String>,
    pub pid: u32,
    pub version: String,
}

impl Health {
    pub fn down() -> Self {
        Self {
            daemon: DaemonState::Down,
            runner_reachable: false,
            last_sync_at: None,
            last_sync_error: None,
            pid: 0,
            version: String::new(),
        }
    }

    pub fn line(&self) -> String {
        let state = match self.daemon {
            DaemonState::Healthy => "healthy",
            DaemonState::Degraded => "degraded",
            DaemonState::Down => return "daemon down".to_owned(),
            DaemonState::Other => "unknown",
        };
        let mut line = format!("daemon {state}");
        if !self.runner_reachable {
            segment(&mut line, "runner unreachable");
        }
        if let Some(error) = &self.last_sync_error {
            segment(&mut line, error);
        }
        line
    }
}

fn segment(line: &mut String, text: &str) {
    line.push_str(" · ");
    // A future reason or runtime label must not turn the health line into multiple lines.
    for ch in text.chars() {
        line.push(if ch.is_control() { ' ' } else { ch });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_health_enum_values_and_fields_do_not_break_clients() {
        let health: Health = serde_json::from_value(serde_json::json!({
            "daemon": "future_state", "runner_reachable": true,
            "last_sync_at": null, "last_sync_error": null,
            "pid":42, "version":"1", "future_field":true
        }))
        .unwrap();
        assert_eq!(health.daemon, DaemonState::Other);
    }
}

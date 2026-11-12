//! Health snapshots are observations, not proof that any requested action completed.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Healthy,
    Degraded,
    Down,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Ok,
    Unreachable,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PmState {
    Live,
    Relaunching,
    Absent,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealth {
    pub kind: String,
    pub state: RuntimeState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopFacts {
    pub pass_seq: u64,
    /// RFC 3339 timestamp, absent before the first completed pass.
    pub last_pass_at: Option<String>,
    pub last_pass_ms: Option<u64>,
    pub age_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub daemon: DaemonState,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub runtimes: Vec<RuntimeHealth>,
    pub pm: PmState,
    #[serde(rename = "loop")]
    pub loop_facts: LoopFacts,
    pub pid: u32,
    pub version: String,
}

impl Health {
    pub fn down() -> Self {
        Self {
            daemon: DaemonState::Down,
            reasons: Vec::new(),
            runtimes: Vec::new(),
            pm: PmState::Absent,
            loop_facts: LoopFacts::default(),
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
        for reason in &self.reasons {
            segment(&mut line, reason);
        }
        for runtime in &self.runtimes {
            let state = match runtime.state {
                RuntimeState::Ok => "ok",
                RuntimeState::Unreachable => "unreachable",
                RuntimeState::Other => "unknown",
            };
            segment(&mut line, &format!("{} {state}", runtime.kind));
        }
        let pm = match self.pm {
            PmState::Live => "live",
            PmState::Relaunching => "relaunching",
            PmState::Absent => "absent",
            PmState::Other => "unknown",
        };
        segment(&mut line, &format!("PM {pm}"));
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
            "daemon": "future_state", "reasons": [], "pm": "future_pm",
            "runtimes": [{"kind":"herdr-tui","state":"future_probe","extra":true}],
            "loop": {"pass_seq":1,"last_pass_at":null,"last_pass_ms":null,"age_secs":null},
            "pid":42, "version":"1", "future_field":true
        }))
        .unwrap();
        assert_eq!(health.daemon, DaemonState::Other);
        assert_eq!(health.pm, PmState::Other);
        assert_eq!(health.runtimes[0].state, RuntimeState::Other);
    }
}

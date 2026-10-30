//! Durable review preflight waits; marker discovery belongs to the Git adapter.
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfraWaitClass {
    Contention,
    Operation,
}

impl InfraWaitClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Contention => "contention",
            Self::Operation => "operation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfraWait {
    pub version: u32,
    pub class: InfraWaitClass,
    pub marker: String,
    pub path: PathBuf,
    pub first_seen: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_mtime: Option<Timestamp>,
    /// The durable send claim. Its presence never implies transport success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttentionLedger, Config, Task};
    use serde_json::json;

    #[test]
    fn legacy_task_defaults_new_fields() {
        let task: Task = serde_json::from_value(json!({
            "id": "T-001",
            "title": "Legacy",
            "status": "review",
            "created": "2026-10-25T00:00:00Z",
            "updated": "2026-10-25T00:00:00Z"
        }))
        .unwrap();
        assert!(task.infra_wait.is_none());
        assert_eq!(task.attention, AttentionLedger::default());
    }

    #[test]
    fn infra_wait_roundtrips_both_classes_and_optional_times() {
        for class in [InfraWaitClass::Contention, InfraWaitClass::Operation] {
            let wait = InfraWait {
                version: 1,
                class,
                marker: "index.lock".into(),
                path: "/repo/.git/index.lock".into(),
                first_seen: Timestamp::from_second(1).unwrap(),
                first_mtime: None,
                held_at: None,
            };
            let value = serde_json::to_value(&wait).unwrap();
            assert_eq!(value["class"], class.as_str());
            assert_eq!(serde_json::from_value::<InfraWait>(value).unwrap(), wait);
        }
    }

    #[test]
    fn infra_wait_configuration_defaults_and_validates_named_bounds() {
        let root = tempfile::tempdir().unwrap();
        let mut config: Config = toml::from_str("").unwrap();
        let policy = config.review_policy(None, None);
        assert_eq!(config.review_infra_wait_secs(), 300);
        for secs in [30, 300, 3600] {
            config.review.infra_wait_secs = Some(secs);
            assert_eq!(config.review_infra_wait_secs(), secs);
            assert!(config.review.is_empty());
            assert!(!config.has_review_policy(None, None));
            assert_eq!(config.review_policy(None, None), policy);
            assert!(
                !config
                    .problems(root.path(), &Default::default())
                    .iter()
                    .any(|problem| problem.contains("infra_wait_secs"))
            );
            let restored: Config = toml::from_str(&config.to_toml()).unwrap();
            assert_eq!(restored.review_infra_wait_secs(), secs);
        }
        for secs in [0, 29, 3601, 1_000_000] {
            let config: Config =
                toml::from_str(&format!("[review]\ninfra_wait_secs = {secs}")).unwrap();
            assert!(
                config
                    .problems(root.path(), &Default::default())
                    .iter()
                    .any(|problem| problem.starts_with("review.infra_wait_secs "))
            );
        }
        let config: Config = toml::from_str(
            "[projects.app]\npath = '.'\n[projects.app.review]\ninfra_wait_secs = 29",
        )
        .unwrap();
        assert!(
            config
                .problems(root.path(), &Default::default())
                .iter()
                .any(|problem| problem.starts_with("projects.app.review.infra_wait_secs "))
        );
    }
}

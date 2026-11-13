//! How a failed command's error reaches stderr; one implementation for the CLI and executors.
use serde_json::json;

const RUN_HINT: &str = "Read current identity with corral run progress <task-id> --json; reconcile stale input before resubmitting.";

/// Each style's `command` is the full command label, e.g. `repair triage` or `run wait`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorStyle {
    /// `error: {friendly_error}`.
    Plain,
    /// `{"command","error"}` (repair, selection).
    Family { command: String },
    /// `{"command","error"}` plus `errors` from `SetupValidationErrors`.
    Setup { command: String },
    /// `run --json`: `{"command","task","error","hint"}`.
    Run { command: String, task: String },
}

/// Exactly the stderr bytes of a failure in `style`, including the trailing newline.
pub fn failure(style: &ErrorStyle, error: &anyhow::Error) -> String {
    match style {
        ErrorStyle::Plain => format!("error: {}\n", corral_herdr::friendly_error(error)),
        ErrorStyle::Family { command } => {
            format!(
                "{}\n",
                json!({"command": command, "error": format!("{error:#}")})
            )
        }
        ErrorStyle::Setup { command } => {
            let mut body = json!({"command": command, "error": format!("{error:#}")});
            if let Some(problems) = error.downcast_ref::<crate::SetupValidationErrors>() {
                body["errors"] = json!(problems.0);
            }
            format!("{body}\n")
        }
        ErrorStyle::Run { command, task } => format!(
            "{}\n",
            json!({
                "command": command,
                "task": task,
                "error": format!("{error:#}"),
                "hint": RUN_HINT,
            })
        ),
    }
}

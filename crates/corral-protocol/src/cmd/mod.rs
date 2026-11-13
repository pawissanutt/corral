//! Typed owner/agent commands. Families live in sibling files.
//!
//! Family rules:
//! - One struct variant per routed leaf; the verb tag is its kebab-case CLI name.
//! - Fields mirror clap args in snake_case, including globals read by the handler.
//! - Paths become `FileInput` or client-merged text; executors never read stdin.
//! - Core enums, identifiers and durations travel as serde/`FromStr` strings.
//! - Entry/environment-derived values come from `Caller`, never variant fields.
//! - Keep `#[serde(other)] Other` last and return `"other"` from its `verb()`.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod assistant;
pub mod attention;
pub mod inbox;
pub mod knowledge;
pub mod meta;
pub mod mode;
pub mod plan;
pub mod pm;
pub mod question;
pub mod queue;
pub mod repair;
pub mod run;
pub mod setup;
pub mod task;

pub const CMD_V1: &str = "cmd.v1";
pub const CMD_SUBMIT: &str = "cmd.submit";
pub const CMD_RESULT: &str = "cmd.result";
/// Record area of the idempotency journal (core record store).
pub const COMMANDS_AREA: &str = "app-commands";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "family", content = "args", rename_all = "snake_case")]
// One value per submitted command; family sizes differ and boxing would change nothing on the wire.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    Assistant(assistant::AssistantCommand),
    Attention(attention::AttentionCommand),
    Inbox(inbox::InboxCommand),
    Knowledge(knowledge::KnowledgeCommand),
    Meta(meta::MetaCommand),
    Mode(mode::ModeCommand),
    Plan(plan::PlanCommand),
    Pm(pm::PmCommand),
    Question(question::QuestionCommand),
    Queue(queue::QueueCommand),
    Repair(repair::RepairCommand),
    Run(run::RunCommand),
    Setup(setup::SetupCommand),
    Task(task::TaskCommand),
    /// A family this build does not know; its `args`, if any, are skipped. A plain
    /// `#[serde(other)]` unit arm of an adjacently tagged enum refuses non-null content.
    #[serde(other, deserialize_with = "skip_args")]
    Other,
}

fn skip_args<'de, D: serde::Deserializer<'de>>(args: D) -> Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(args).map(|_| ())
}

impl Command {
    /// `family.verb`, e.g. `question.answer`; `other` for unknown arms. Timing/marker label.
    pub fn name(&self) -> String {
        let (family, verb) = match self {
            Self::Assistant(command) => ("assistant", command.verb()),
            Self::Attention(command) => ("attention", command.verb()),
            Self::Inbox(command) => ("inbox", command.verb()),
            Self::Knowledge(command) => ("knowledge", command.verb()),
            Self::Meta(command) => ("meta", command.verb()),
            Self::Mode(command) => ("mode", command.verb()),
            Self::Plan(command) => ("plan", command.verb()),
            Self::Pm(command) => ("pm", command.verb()),
            Self::Question(command) => ("question", command.verb()),
            Self::Queue(command) => ("queue", command.verb()),
            Self::Repair(command) => ("repair", command.verb()),
            Self::Run(command) => ("run", command.verb()),
            Self::Setup(command) => ("setup", command.verb()),
            Self::Task(command) => ("task", command.verb()),
            Self::Other => return "other".into(),
        };
        if verb == "other" {
            "other".into()
        } else {
            format!("{family}.{verb}")
        }
    }

    /// False for unknown families or verbs.
    pub fn is_known(&self) -> bool {
        match self {
            Self::Assistant(command) => command.verb() != "other",
            Self::Attention(command) => command.verb() != "other",
            Self::Inbox(command) => command.verb() != "other",
            Self::Knowledge(command) => command.verb() != "other",
            Self::Meta(command) => command.verb() != "other",
            Self::Mode(command) => command.verb() != "other",
            Self::Plan(command) => command.verb() != "other",
            Self::Pm(command) => command.verb() != "other",
            Self::Question(command) => command.verb() != "other",
            Self::Queue(command) => command.verb() != "other",
            Self::Repair(command) => command.verb() != "other",
            Self::Run(command) => command.verb() != "other",
            Self::Setup(command) => command.verb() != "other",
            Self::Task(command) => command.verb() != "other",
            Self::Other => false,
        }
    }
}

/// A file named on the command line, read by the client before routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileInput {
    /// Path as typed (`Path::display()`), for messages only.
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientBuild {
    /// Absolute path of the client's executable.
    pub exe: String,
    /// The client's build identity (what `corral build-identity` prints).
    pub build: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expect {
    /// Refuse with `conflict` unless `corral_core::owner_revision(task)` still equals `revision`.
    TaskRevision { task: String, revision: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CmdSubmit {
    pub cmd_id: String,
    pub caller: Caller,
    /// Required from local CLI clients; must equal the daemon's (`pm_read::client_identity`).
    pub client_build: Option<ClientBuild>,
    #[serde(default)]
    pub expect: Option<Expect>,
    /// RFC 3339.
    pub issued_at: String,
    /// RFC 3339; CLI: issued_at + 120 s.
    pub expires_at: String,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CmdResultQuery {
    pub cmd_id: String,
    /// Wait for an in-flight command, at most 25_000.
    pub wait_ms: u32,
    /// For an absent id, write an abandoned tombstone that refuses late submits.
    pub abandon_if_absent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CmdState {
    Done,
    Failed,
    Conflict,
    Busy,
    Running,
    Unknown,
    Abandoned,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CmdError {
    /// Leading `snake_case:` message token, else `conflict`, `busy` or `failed`.
    pub code: String,
    /// The error formatted with its full context chain (`format!("{err:#}")`).
    pub message: String,
}

/// Exactly `frames` DATA frames follow on `stream`: one UTF-8 JSON `CmdOutput`, `bytes` long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutputBody {
    pub stream: u32,
    pub bytes: u64,
    pub frames: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CmdResult {
    pub cmd_id: String,
    pub state: CmdState,
    /// Process exit status the CLI uses: 0 for `done`, 1 otherwise.
    pub exit: u8,
    pub error: Option<CmdError>,
    /// None for `running`, `unknown`, `abandoned`, and replays without retained output.
    pub output: Option<OutputBody>,
    pub replayed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Principal {
    /// What clients send; the daemon replaces it from the transport.
    #[default]
    Unverified,
    LocalUser {
        uid: u32,
    },
    /// Reserved for paired devices; refused on the Unix socket.
    Device {
        id: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Cli,
    Board,
    PmHost,
    App,
    #[serde(other)]
    Other,
}

/// `Entry::via` today, minus `owner_action` (owner-action children never route).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Cli,
    Pm,
    Hook,
    Tui,
    Assistant,
    App,
    #[serde(other)]
    Other,
}

/// Mirrors `corral_core::Actor` serde names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActorName {
    Owner,
    Pm,
    Agent,
    Assistant,
    Autopilot,
    Reviewer,
    System,
    #[serde(other)]
    Other,
}

/// Caller-identity variables, `None` = unset. Values are `to_string_lossy` of `var_os`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallerEnv {
    pub corral_pm: Option<String>,
    pub corral_assistant: Option<String>,
    pub ai_agent: Option<String>,
    pub pi_coding_agent: Option<String>,
    pub corral_agent: Option<String>,
    pub corral_reviewer: Option<String>,
    pub corral_agent_task: Option<String>,
    pub corral_agent_bootstrap: Option<String>,
    pub corral_task: Option<String>,
    pub corral_herdr_session: Option<String>,
    pub herdr_env: Option<String>,
    pub herdr_socket_path: Option<String>,
    pub herdr_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Caller {
    #[serde(default)]
    pub principal: Principal,
    pub client: ClientKind,
    /// `Entry::actor`: explicit `--author/--origin`, hook/pm/tui defaults.
    pub actor: Option<ActorName>,
    pub via: Via,
    /// Validated `CORRAL_ASSISTANT` session id when `via = assistant`.
    pub assistant: Option<String>,
    pub planning_safe: bool,
    /// Raw `std::env::current_dir()`; None when unreadable or not UTF-8.
    pub cwd: Option<String>,
    pub env: CallerEnv,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_family_and_unknown_verb_decode_to_other() {
        let family: Command = serde_json::from_value(serde_json::json!({
            "family": "future", "args": {"verb": "launch", "id": "T-1"}
        }))
        .unwrap();
        assert_eq!(family, Command::Other);
        assert!(!family.is_known());
        let verb: Command = serde_json::from_value(serde_json::json!({
            "family": "task", "args": {"verb": "teleport", "id": "T-1"}
        }))
        .unwrap();
        assert_eq!(verb, Command::Task(task::TaskCommand::Other));
        assert!(!verb.is_known());
        let comment = Command::Task(task::TaskCommand::Comment {
            id: "T-1".into(),
            text: "hello".into(),
            author: "owner".into(),
            send: false,
        });
        let wire = serde_json::to_value(&comment).unwrap();
        assert_eq!(wire["family"], "task");
        assert_eq!(wire["args"]["verb"], "comment");
        assert_eq!(serde_json::from_value::<Command>(wire).unwrap(), comment);
        assert!(comment.is_known());
    }
}

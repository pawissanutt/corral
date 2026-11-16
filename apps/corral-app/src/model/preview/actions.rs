mod questions;
mod session_ops;
mod settings;
mod tasks;
mod workflow;

use questions::{attention, question};
use session_ops::session;
use settings::config;
use tasks::{message, task};
use workflow::{annotation, plan, review};

use super::{change, content, control, put, sessions, stamp};
use crate::model::{Act, daemon::*};
use corral_protocol::{
    app::*,
    cmd::{
        self, Command, annotation::AnnotationCommand as Annotation,
        attention::AttentionCommand as Attention, config::ConfigCommand as Config,
        plan::PlanCommand as Plan, question::QuestionCommand as Question,
        review::ReviewCommand as Review, session::SessionCommand as Session,
        task::TaskCommand as TaskCmd,
    },
    detail::*,
    rows::*,
};

pub fn act(feed: &mut Feed, act: &Act) -> Result<(), String> {
    let request = serde_json::to_value(&act.command).map_err(|error| error.to_string())?;
    if let Some(previous) = feed.commands.get(&act.key) {
        if previous.request != request {
            return Err(
                "conflict: preview command identity was reused for a different command".into(),
            );
        }
        return if previous.status == CommandStatus::Done {
            Ok(())
        } else {
            Err(previous
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "Preview command already settled".into()))
        };
    }
    let result = authorize(feed, act).and_then(|()| execute(feed, &act.command));
    let error = result.as_ref().err().map(|message| ClientError {
        code: if message.starts_with("conflict:") {
            "conflict"
        } else {
            "preview_refused"
        }
        .into(),
        message: message.clone(),
    });
    let status = if result.is_ok() {
        CommandStatus::Done
    } else if error.as_ref().is_some_and(|error| error.code == "conflict") {
        CommandStatus::Conflict
    } else {
        CommandStatus::Refused
    };
    feed.commands.insert(
        act.key.clone(),
        CommandState {
            device: Some("preview-device".into()),
            method: ACT_SUBMIT.into(),
            request,
            status,
            result: Some(cmd::CmdResult {
                cmd_id: act.key.to_string(),
                state: if result.is_ok() {
                    cmd::CmdState::Done
                } else if status == CommandStatus::Conflict {
                    cmd::CmdState::Conflict
                } else {
                    cmd::CmdState::Failed
                },
                exit: u8::from(result.is_err()),
                error: error.as_ref().map(|error| cmd::CmdError {
                    code: error.code.clone(),
                    message: error.message.clone(),
                }),
                output: None,
                replayed: false,
            }),
            error,
            entry: None,
        },
    );
    if result.is_ok() {
        content::refresh_plans(feed);
        super::history::refresh_filings(feed);
        super::projects::refresh(feed);
        change(feed, None, act.title.clone(), ChangeKind::Other);
    }
    result
}

fn authorize(feed: &Feed, act: &Act) -> Result<(), String> {
    let authority = &feed
        .session
        .as_ref()
        .ok_or("Preview device is not enrolled")?
        .authority;
    let allowed = match act.command.app_class() {
        ActClass::Reply => authority.reply,
        ActClass::Act | ActClass::Confirm => authority.act,
        ActClass::Admin => authority.admin,
        _ => false,
    };
    if !allowed {
        return Err("Preview device authority refuses this command".into());
    }
    if let Some(cmd::Expect::TaskRevision { task, revision }) = &act.expect {
        let expected = feed
            .detail(&DetailRef::Task { id: task.clone() })
            .and_then(|cache| cache.body());
        if !matches!(expected, Some(DetailBody::Task(detail)) if &detail.owner_revision == revision)
        {
            return Err("conflict: preview task owner revision changed".into());
        }
    }
    Ok(())
}

fn execute(feed: &mut Feed, command: &Command) -> Result<(), String> {
    match command {
        Command::Question(command) => question(feed, command),
        Command::Task(command) => task(feed, command),
        Command::Session(command) => session(feed, command),
        Command::Plan(command) => plan(feed, command),
        Command::Review(command) => review(feed, command),
        Command::Annotation(command) => annotation(feed, command),
        Command::Config(command) => config(feed, command),
        Command::Attention(command) => attention(feed, command),
        Command::Queue(command) => super::control::queue(feed, command),
        Command::Assistant(command) => super::control::permissions(feed, command),
        Command::Mode(command) => {
            use cmd::mode::ModeCommand;
            let mode = match command {
                ModeCommand::Assist {} | ModeCommand::Pause {} | ModeCommand::Off { .. } => {
                    OperatingMode::Assist
                }
                ModeCommand::Autopilot { .. }
                | ModeCommand::Resume {}
                | ModeCommand::Turbo { .. } => OperatingMode::Autopilot,
                _ => return unsupported(command.verb()),
            };
            for settings in feed.settings.values_mut() {
                settings.mode = mode;
                settings.rev += 1;
            }
            Ok(())
        }
        Command::Pm(cmd::pm::PmCommand::InboxPost { body, .. }) => {
            sessions::append(feed, "PM", body)
        }
        Command::Message(command) => message(feed, command),
        _ => unsupported(&command.name()),
    }
}

fn unsupported<T>(name: &str) -> Result<T, String> {
    Err(format!(
        "Preview does not simulate {name}; nothing was sent or changed."
    ))
}

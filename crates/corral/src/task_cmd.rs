//! Task verbs routed through the shared command executor.
use anyhow::{Result, bail};
use corral_core::Status;
use corral_protocol::cmd::{Command, task::TaskCommand};

use crate::Cmd;
use crate::cmds::route;
use crate::invocation::Invocation;

/// Runs one of the task verbs `cmds::run` hands over; any other command is a wiring error.
pub(crate) fn run(inv: &Invocation, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::New {
            origin,
            title,
            project,
            idea,
            ready,
            body,
            body_file,
            profile,
            depends,
            priority,
            kind,
            size,
            difficulty,
            design,
            labels,
        } => {
            let status = if idea {
                Status::Idea
            } else if ready {
                Status::Ready
            } else {
                Status::Backlog
            };
            crate::idea_cmd::new_task(
                inv,
                &title,
                project.as_deref(),
                status,
                body.as_deref(),
                body_file.as_deref(),
                profile,
                &depends,
                priority,
                kind,
                size,
                difficulty,
                origin,
                design,
                &labels,
            )
        }
        Cmd::Comment {
            id,
            text,
            author,
            send,
        } => inv.submit(Command::Task(TaskCommand::Comment {
            id,
            text,
            author: author.as_str().into(),
            send,
        })),
        Cmd::Move {
            id,
            status,
            force,
            owner_ok,
            reason,
            author: _,
        } => inv.submit(Command::Task(TaskCommand::Move {
            id,
            status,
            force,
            owner_ok,
            reason,
        })),
        Cmd::Idea(args) => crate::idea_cmd::run(inv, args),
        Cmd::Route {
            id,
            set_reviewer,
            clear_reviewer,
            profile,
            json,
        } => {
            if set_reviewer.is_none()
                && !clear_reviewer
                && profile.profile.is_none()
                && profile.model.is_none()
                && profile.effort.is_none()
            {
                return route::explain(inv.ctx()?, &id, &profile.choice(), json);
            }
            inv.submit(Command::Task(TaskCommand::Route {
                id,
                set_reviewer,
                clear_reviewer,
                profile: profile.profile,
                model: profile.model,
                effort: profile.effort,
                json,
            }))
        }
        Cmd::Hold { id, reason } => inv.submit(Command::Task(TaskCommand::Hold { id, reason })),
        Cmd::Release { id } => inv.submit(Command::Task(TaskCommand::Release { id })),
        Cmd::WaiveReview { id, reason } => {
            inv.submit(Command::Task(TaskCommand::WaiveReview { id, reason }))
        }
        Cmd::Focus { id, all, clear: _ } => {
            inv.submit(Command::Task(TaskCommand::Focus { id, all }))
        }
        Cmd::RecoverInfrastructure { id } => {
            inv.submit(Command::Task(TaskCommand::RecoverInfrastructure { id }))
        }
        _ => bail!("internal error: not a task command"),
    }
}

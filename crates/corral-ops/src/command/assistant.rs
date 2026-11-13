//! `assistant` command executors.
use anyhow::Result;
use corral_protocol::cmd::assistant::AssistantCommand;

use super::{Exec, Verb};

impl Verb for AssistantCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Create {
                profile,
                focus,
                pane,
                herdr_socket,
                json,
            } => {
                let session = cx.ctx.store.create_assistant(
                    &profile,
                    focus.as_deref(),
                    pane.as_deref(),
                    herdr_socket,
                )?;
                if json {
                    cx.out.json_pretty(&session)?;
                } else {
                    cx.out.line(format_args!("{}", session.id));
                }
                Ok(())
            }
            Self::Summary { id, file, json } => {
                let body = file.text;
                let session = cx.ctx.store.update_assistant(&id, |session| {
                    session.summary = body.lines().next().unwrap_or("").to_owned();
                    session.body = body;
                    Ok(())
                })?;
                cx.ctx.store.log_event(
                    None,
                    "assistant_summary",
                    serde_json::json!({"session": id}),
                )?;
                if json {
                    cx.out.json_pretty(&session)?;
                } else {
                    cx.out.line(format_args!("{} summary saved", session.id));
                }
                Ok(())
            }
            Self::PermissionsRevoke {
                id,
                grant,
                all,
                json,
            } => {
                let grant = if all { None } else { grant };
                let by = cx
                    .invoker
                    .caller
                    .env
                    .corral_assistant
                    .as_deref()
                    .unwrap_or("owner");
                let done = crate::assistant::permissions::revoke(
                    &cx.ctx.store,
                    &id,
                    grant.as_deref(),
                    by,
                )?;
                if let Some(warning) = &done.warning {
                    cx.out.err_line(format_args!("{warning}"));
                }
                if json {
                    return cx.out.json_pretty(&done);
                }
                if done.revoked.is_empty() {
                    cx.out
                        .line(format_args!("{id}: no active write grant to revoke"));
                } else {
                    cx.out.line(format_args!(
                        "{id}: revoked {} (by {by})",
                        done.revoked.join(", ")
                    ));
                }
                Ok(())
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }
}

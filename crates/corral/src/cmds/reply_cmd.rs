use anyhow::{Context, Result};
use corral_core::CommentAuthor;
use corral_ops::Ctx;

use super::route;

pub(super) fn run(
    ctx: &Ctx,
    id: &str,
    text: &[String],
    resend: bool,
    mark_delivered: bool,
    reason: Option<&str>,
    author: Option<CommentAuthor>,
) -> Result<()> {
    let task = if mark_delivered {
        corral_ops::mark_reply_delivered(
            ctx,
            id,
            reason.context("--mark-delivered requires --reason")?,
            author.context("--mark-delivered requires --author owner|pm")?,
        )?
    } else if resend {
        corral_ops::resend_reply(ctx, id)?
    } else {
        corral_ops::reply(ctx, id, &text.join(" "))?
    };
    if task
        .current_run()
        .is_some_and(|run| run.pending_reply.is_some())
    {
        println!(
            "{} reply not delivered; pending (use reply {} --resend)",
            task.id, task.id
        );
    } else if mark_delivered {
        println!(
            "{} reply marked delivered ({})",
            task.id,
            task.status.label()
        );
    } else {
        println!("{} replied ({})", task.id, task.status.label());
    }
    Ok(())
}

pub(super) fn changes(ctx: &Ctx, id: &str, text: &[String], escalate: bool) -> Result<()> {
    if escalate {
        route::print_outcome(corral_ops::request_changes_escalated(
            ctx,
            id,
            &text.join(" "),
        )?);
    } else {
        let task = corral_ops::request_changes(ctx, id, &text.join(" "))?;
        println!("{} -> {}", task.id, task.status.label());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn mark_delivered_requires_attribution_and_excludes_delivery_modes() {
        // Given complete audited acknowledgements from either supported author.
        for author in ["owner", "pm"] {
            // When the reply CLI is parsed.
            let cli = crate::Cli::try_parse_from([
                "corral",
                "reply",
                "T-001",
                "--mark-delivered",
                "--reason",
                "Verified in session",
                "--author",
                author,
            ])
            .unwrap();
            // Then no reply text or resend is required.
            assert!(matches!(
                cli.cmd,
                Some(crate::Cmd::Reply {
                    mark_delivered: true,
                    resend: false,
                    reason: Some(_),
                    author: Some(_),
                    ..
                })
            ));
        }
        // Given incomplete or conflicting acknowledgement arguments.
        for args in [
            vec!["--mark-delivered"],
            vec!["--mark-delivered", "--reason", "Verified"],
            vec!["--mark-delivered", "--author", "owner"],
            vec![
                "--mark-delivered",
                "--reason",
                "Verified",
                "--author",
                "agent",
            ],
            vec![
                "--mark-delivered",
                "--reason",
                "Verified",
                "--author",
                "owner",
                "--resend",
            ],
            vec![
                "--mark-delivered",
                "--reason",
                "Verified",
                "--author",
                "owner",
                "New text",
            ],
            vec!["--reason", "Verified", "New text"],
            vec!["--author", "pm", "--resend"],
        ] {
            let mut command = vec!["corral", "reply", "T-001"];
            command.extend(args);
            // When parsed, then the conflicting or missing arguments are rejected.
            assert!(crate::Cli::try_parse_from(command).is_err());
        }
    }
}

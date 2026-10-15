//! Append-only owner context and optional delivery through the durable reply path.
use anyhow::{Context, Result, bail};
use corral_core::{CommentAuthor, Status, Task, TaskComment};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// Saves a note at any status. Delivery failures leave both the note and pending reply intact.
pub fn comment(ctx: &Ctx, id: &str, text: &str, author: CommentAuthor, send: bool) -> Result<Task> {
    if text.trim().is_empty() {
        bail!("comment text must not be empty");
    }
    let note = TaskComment {
        author,
        at: Timestamp::now(),
        text: text.to_string(),
    };
    let task = ctx.store.update(id, |task| {
        if send
            && (!matches!(task.status, Status::Running | Status::NeedsMe)
                || task
                    .current_run()
                    .is_none_or(|r| r.outcome.is_some() || r.herdr.is_none()))
        {
            bail!("{} has no running agent to send a comment to", task.id);
        }
        task.comments.push(note.clone());
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "commented",
        json!({"comment": note, "send": send}),
    )?;
    if send {
        let message = format!(
            "Owner note for {} (author: {}, at: {}). Read the durable comment in the task file {} \
             (comments entry {}), then apply it as owner context alongside the task specification.\n\n{}",
            task.id,
            author.as_str(),
            note.at,
            task.path.display(),
            task.comments.len(),
            note.text,
        );
        return crate::reply(ctx, &task.id, &message)
            .context("comment saved; agent delivery failed");
    }
    Ok(task)
}

/// Appended independently of custom templates so owner context is never omitted.
pub(crate) fn owner_comments(task: &Task) -> String {
    if task.comments.is_empty() {
        return String::new();
    }
    let mut text = format!(
        "\n## Owner comments\n\nDurable source: {}\n",
        task.path.display()
    );
    for comment in &task.comments {
        text.push_str(&format!(
            "\n### {} · {}\n\n{}\n",
            comment.author.as_str(),
            comment.at,
            comment.text,
        ));
    }
    text
}

#[cfg(test)]
#[path = "comments_tests.rs"]
mod tests;

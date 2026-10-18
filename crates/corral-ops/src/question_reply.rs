//! Request-bound OmO replies. A prepared route is durable before any delivery;
//! a native answer never falls back to terminal input, including after a lost ack.
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::Task;
use corral_herdr::{Delivery, DeliveryVia};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use serde_json::json;

use crate::Ctx;

#[cfg(all(test, unix))]
mod tests;
mod transcript;
#[cfg(unix)]
mod transport;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Question {
    socket: PathBuf,
    pid: u32,
    instance: String,
    session: PathBuf,
    session_id: String,
    request: String,
    identity: String,
}

#[derive(Serialize, Deserialize)]
struct Prepared {
    run: String,
    pane: String,
    text: String,
    question: Option<Question>,
}

struct Target<'a> {
    run: &'a str,
    pane: &'a str,
    harness: &'a str,
    cwd: PathBuf,
}

fn target<'a>(ctx: &Ctx, task: &'a Task) -> Result<Target<'a>> {
    if let Some(run) = task.current_run().filter(|run| run.outcome.is_none()) {
        return Ok(Target {
            run: &run.id,
            pane: &run.herdr.as_ref().context("reply has no pane")?.pane_id,
            harness: run.harness.as_deref().unwrap_or(&run.agent),
            cwd: run.worktree.clone(),
        });
    }
    let run = task.plan_runs.last().context("reply has no live target")?;
    Ok(Target {
        run: &run.id,
        pane: &run
            .herdr
            .as_ref()
            .context("planner reply has no pane")?
            .pane_id,
        harness: &run.harness,
        cwd: ctx
            .store
            .project_path(task.project.as_deref().context("planner has no project")?)?,
    })
}

pub(crate) fn deliver(ctx: &Ctx, task: &Task, text: &str, resend: bool) -> Result<Delivery> {
    let target = target(ctx, task)?;
    if target.harness != "omo" {
        return ctx.herdr.agent_prompt(target.pane, text);
    }
    let mut delivery = Delivery {
        via: DeliveryVia::OmoQuestion,
        submitted: false,
        attempts: 0,
        error: None,
    };
    let result = (|| {
        let pane = target.pane;
        let prepared = if resend {
            ctx.store
                .events()?
                .into_iter()
                .rev()
                .find(|event| {
                    event.task.as_deref() == Some(&task.id)
                        && event.kind == "reply_prepared"
                        && event.detail["run"] == target.run
                })
                .map(|event| serde_json::from_value::<Prepared>(event.detail))
                .transpose()
                .context("reading prepared reply")?
        } else {
            None
        };
        let question = match prepared {
            Some(prepared) => {
                ensure!(
                    prepared.pane == pane && prepared.text == text,
                    "prepared reply target changed; inspect the pending reply manually"
                );
                prepared.question
            }
            None => {
                let question = discover(ctx, task)?;
                // Legacy pending text was never bound to this question. Do not guess.
                ensure!(
                    !resend || question.is_none(),
                    "pending reply has no native request binding; answer the exact question manually"
                );
                ctx.store.log_event(
                    Some(&task.id),
                    "reply_prepared",
                    serde_json::to_value(Prepared {
                        run: target.run.into(),
                        pane: pane.into(),
                        text: text.into(),
                        question: question.clone(),
                    })?,
                )?;
                question
            }
        };
        match question {
            Some(question) => {
                let snapshot = transcript::Snapshot::read(&question.session)?;
                snapshot.check_session(&question)?;
                if snapshot.answered(&question, text) {
                    delivery.submitted = true;
                    return Ok(());
                }
                ensure!(
                    snapshot.pending.get(&question.request) == Some(&question.identity),
                    "native question changed or settled without this answer; inspect request {} manually",
                    question.request
                );
                #[cfg(unix)]
                {
                    transport::check_pane(ctx, task, &question)?;
                    let mut connection = transport::Connection::open(&question.socket)?;
                    connection.check(&question)?;
                    // A native comment answers the complete request without guessing which
                    // menu option a free-form corral reply was meant to select.
                    delivery.attempts = 1;
                    connection.call(json!({
                        "type":"extension_ui_response", "uiRequestId":question.request,
                        "answers":{}, "comment":text,
                    }))?;
                    delivery.submitted = true;
                }
                #[cfg(not(unix))]
                anyhow::bail!(
                    "native question transport unsupported on this platform; answer manually"
                );
            }
            None => {
                // Even a prepared ordinary reply must not become an answer to a
                // question which appeared after preparation.
                ensure!(
                    discover(ctx, task)?.is_none(),
                    "a native question replaced the ordinary composer; inspect the pending reply manually"
                );
                delivery = ctx.herdr.agent_prompt(pane, text)?;
            }
        }
        Ok::<_, anyhow::Error>(())
    })();
    if let Err(error) = result {
        delivery.error = Some(format!("{error:#}"));
    }
    Ok(delivery)
}

fn discover(ctx: &Ctx, task: &Task) -> Result<Option<Question>> {
    #[cfg(unix)]
    if let Some(question) = transport::discover(ctx, task)? {
        return Ok(Some(question));
    }
    let target = target(ctx, task)?;
    ensure!(
        crate::pm_questions::pending_at(ctx, &target.cwd)?.is_none(),
        "native question has no supported request-bound endpoint; answer it manually (do not paste or guess Enter keys)"
    );
    Ok(None)
}

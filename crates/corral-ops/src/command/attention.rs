//! `attention` command executors.
use anyhow::Result;
use corral_protocol::cmd::attention::AttentionCommand;

use super::{Exec, OpenSpec, Verb};

impl Verb for AttentionCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Attend {
                key,
                note,
                next_check,
                request,
                json,
            } => {
                let now = jiff::Timestamp::now();
                let request = request.unwrap_or_else(|| format!("attend-{now}"));
                let handling = crate::attention::attend(
                    &cx.ctx, &key, &note, next_check, &request, now, cx.invoker,
                )?;
                if json {
                    cx.out.json_pretty(&handling)?;
                } else {
                    cx.out.line(format_args!(
                        "{key}: handling revision {}, next check {}",
                        handling.revision, handling.next_check
                    ));
                }
                Ok(())
            }
            Self::Park {
                id,
                reason,
                resume_when,
                resume_action,
                review_by,
                json,
            } => {
                let resume_when = resume_when
                    .iter()
                    .map(|value| corral_core::parse_park_check(value))
                    .collect::<Result<Vec<_>>>()?;
                let park = crate::park::park(
                    &cx.ctx,
                    &id,
                    &reason,
                    &resume_when,
                    &resume_action,
                    review_by,
                    jiff::Timestamp::now(),
                    cx.invoker,
                )?;
                if json {
                    cx.out.json_pretty(&park)?;
                } else {
                    cx.out.line(format_args!(
                        "{id}: parked by {} at revision {}, review by {}",
                        park.by.label(),
                        park.revision,
                        park.review_by
                    ));
                }
                Ok(())
            }
            Self::Unpark {
                id,
                expected_revision,
                json,
            } => {
                crate::park::unpark(&cx.ctx, &id, expected_revision, cx.invoker)?;
                if json {
                    cx.out.json(&serde_json::json!({
                        "task": id,
                        "unparked": expected_revision
                    }))?;
                } else {
                    cx.out
                        .line(format_args!("{id}: unparked revision {expected_revision}"));
                }
                Ok(())
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }

    fn open(&self) -> OpenSpec {
        OpenSpec::UNOBSERVED
    }
}

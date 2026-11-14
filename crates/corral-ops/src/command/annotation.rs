//! Typed candidate annotations retain canonical candidate, revision and keyed-replay checks.
use super::{Exec, Verb};
use crate::annotations::{
    self, AnnotationActor, AnnotationAuthor, AnnotationSurface, ChangeAnnotation, OpenAnnotation,
};
use anyhow::Result;
use corral_protocol::cmd::annotation::{AnnotationCommand, AnnotationSide};

impl Verb for AnnotationCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let invoker = cx.invoker;
        let actor = if cx.invoker.is_app() {
            AnnotationActor::authenticated(|store, _| {
                let device = invoker.device()?;
                store.annotation_owner_admission()?;
                Ok(AnnotationAuthor::Owner {
                    version: 1,
                    surface: AnnotationSurface::App {
                        device: device.id.clone(),
                        name: device.name.clone(),
                    },
                })
            })
        } else {
            AnnotationActor::owner_cli(cx.invoker.cwd()?)
        };
        let result = match self {
            Self::Open {
                id,
                key,
                expected_run,
                base,
                head,
                path,
                side,
                range,
                text,
            } => {
                let side = match side {
                    AnnotationSide::New => annotations::AnnotationSide::New,
                    AnnotationSide::Old => annotations::AnnotationSide::Old,
                    AnnotationSide::Other => anyhow::bail!("unknown annotation side"),
                };
                let range = range.map(|range| annotations::LineRange {
                    start: range.start,
                    end: range.end,
                });
                annotations::open(
                    &cx.ctx.store,
                    &actor,
                    &id,
                    &key,
                    OpenAnnotation {
                        expected_run,
                        base,
                        head,
                        path,
                        side,
                        range,
                        text,
                    },
                )?
            }
            Self::Reply {
                id,
                key,
                thread,
                expected_revision,
                text,
            } => annotations::reply(
                &cx.ctx.store,
                &actor,
                &id,
                &key,
                ChangeAnnotation {
                    thread,
                    expected_revision,
                    text,
                },
            )?,
            Self::Resolve {
                id,
                key,
                thread,
                expected_revision,
                text,
            } => annotations::resolve(
                &cx.ctx.store,
                &actor,
                &id,
                &key,
                ChangeAnnotation {
                    thread,
                    expected_revision,
                    text,
                },
            )?,
            Self::Reopen {
                id,
                key,
                thread,
                expected_revision,
                text,
            } => annotations::reopen(
                &cx.ctx.store,
                &actor,
                &id,
                &key,
                ChangeAnnotation {
                    thread,
                    expected_revision,
                    text,
                },
            )?,
            Self::Other => return Err(super::unknown_command("other")),
        };
        cx.out.json(&result)
    }
}

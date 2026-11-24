use std::{fs::File, io::ErrorKind};

use anyhow::{Context, Result, ensure};
use corral_engine::Engine;
use corral_protocol::detail::*;

use super::{
    NotFound,
    details::{self, ArtifactSource},
    fingerprint, transcript,
};
use crate::{assistant, sessions, state::Shared};

pub(super) fn detail(
    shared: &Shared,
    engine: &Engine,
    query: &DetailGet,
) -> Result<Option<DetailReply>> {
    let body = match &query.target {
        DetailRef::Session { id, .. } => {
            let session = sessions::resolve(shared, id)?;
            let (path, host, reply_refusal) = session.transcript_view();
            let scope = session.transcript_scope();
            let page = if let Some(path) = path {
                match transcript::page(&session.id, &path, scope, query.page.as_deref()) {
                    Err(error)
                        if query.page.is_none()
                            && error
                                .downcast_ref::<std::io::Error>()
                                .is_some_and(|err| err.kind() == ErrorKind::NotFound) =>
                    {
                        None
                    }
                    result => Some(result?),
                }
            } else {
                ensure!(
                    query.page.is_none(),
                    "invalid: session has no transcript to page"
                );
                None
            };
            let replyable = reply_refusal.is_none();
            let etag = fingerprint(&(
                &query.target,
                page.as_ref().map(|page| &page.stamp),
                &session.id,
                session.role,
                &session.task,
                &session.run,
                host,
                replyable,
                &reply_refusal,
            ))?;
            let (messages, older) = page
                .map(|page| (page.messages, page.older))
                .unwrap_or_default();
            let body = DetailBody::Session(SessionPage {
                id: session.id,
                role: session.role.into(),
                task: session.task,
                run: session.run,
                machine: None,
                host,
                messages,
                older: older.clone(),
                replyable,
                reply_refusal,
            });
            return details::reply(query, engine.store.latest_seq()?, etag, body, older).map(Some);
        }
        DetailRef::SessionItem { id, token } => {
            ensure!(
                query.page.is_none(),
                "invalid: request the next session item fragment by token"
            );
            let session = sessions::resolve(shared, id)?;
            let (path, _, _) = session.transcript_view();
            let path = path.context(NotFound("session has no retained transcript"))?;
            DetailBody::SessionItem(transcript::item(
                &session.id,
                &path,
                session.transcript_scope(),
                token,
            )?)
        }
        DetailRef::Attachment { entry, id } => {
            let path = assistant::attachment_path(shared, entry, id)?;
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    return Err(NotFound("attachment not found").into());
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("open attachment {}", path.display()));
                }
            };
            let bytes = file.metadata().context("stat chat attachment")?.len();
            return details::artifact_page(
                query,
                engine.store.latest_seq()?,
                file,
                ArtifactSource {
                    media_type: None,
                    text: None,
                    fingerprint: fingerprint(&query.target)?,
                    bytes,
                    task: sessions::attachment_scope(entry)?.into(),
                    key: id.clone(),
                },
            )
            .map(Some);
        }
        _ => return Ok(None),
    };
    details::reply(
        query,
        engine.store.latest_seq()?,
        fingerprint(&(&query.target, &body))?,
        body,
        None,
    )
    .map(Some)
}

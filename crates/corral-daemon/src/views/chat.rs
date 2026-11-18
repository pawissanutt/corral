use std::{
    fs::{self, File},
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use corral_engine::Engine;
use corral_protocol::{
    assistant::{EntryKind, MessageState, ThreadEntry, TurnCompletion},
    detail::*,
};

use super::{
    NotFound, Rows,
    details::{self, ArtifactSource},
    fingerprint, transcript,
};
use crate::{assistant, sessions, state::Shared};

pub(super) fn detail(
    shared: &Shared,
    engine: &Engine,
    query: &DetailGet,
) -> Result<Option<DetailReply>> {
    let (body, next) = match &query.target {
        DetailRef::Session { id, .. } => {
            let session = sessions::resolve(shared, id)?;
            let (path, reply_refusal) = session.transcript_view();
            let (messages, older) = if let Some(path) = path {
                match transcript::page(id, &path, query.page.as_deref()) {
                    Err(error)
                        if query.page.is_none()
                            && error
                                .downcast_ref::<std::io::Error>()
                                .is_some_and(|err| err.kind() == ErrorKind::NotFound) =>
                    {
                        (Vec::new(), None)
                    }
                    result => result?,
                }
            } else {
                ensure!(
                    query.page.is_none(),
                    "invalid: session has no transcript to page"
                );
                (Vec::new(), None)
            };
            (
                DetailBody::Session(SessionPage {
                    id: session.id,
                    role: session.role.into(),
                    task: session.task,
                    run: session.run,
                    machine: None,
                    messages,
                    older: older.clone(),
                    replyable: reply_refusal.is_none(),
                    reply_refusal,
                }),
                older,
            )
        }
        DetailRef::SessionItem { id, token } => {
            ensure!(
                query.page.is_none(),
                "invalid: request the next session item fragment by token"
            );
            let (path, _) = sessions::resolve(shared, id)?.transcript_view();
            let path = path.context(NotFound("session has no retained transcript"))?;
            (
                DetailBody::SessionItem(transcript::item(id, &path, token)?),
                None,
            )
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
        next,
    )
    .map(Some)
}

#[derive(PartialEq, Eq)]
pub(crate) struct ThreadStamp {
    path: PathBuf,
    inode: u64,
    bytes: u64,
    mtime: (i64, i64),
}

pub(crate) fn thread(
    shared: &Shared,
    scope: &str,
    previous: Option<&Option<ThreadStamp>>,
) -> Result<Option<(Rows, Option<ThreadStamp>)>> {
    let mut rows = Vec::new();
    let session = match assistant::resolve(shared, scope) {
        Ok(session) => Some(session),
        Err(error) if error.is::<NotFound>() => None,
        Err(error) => return Err(error),
    };
    let stamp = session
        .as_ref()
        .and_then(|session| session.transcript.as_ref())
        .map(|path| match fs::metadata(path) {
            Ok(meta) => Ok(Some(ThreadStamp {
                path: path.clone(),
                inode: meta.ino(),
                bytes: meta.len(),
                mtime: (meta.mtime(), meta.mtime_nsec()),
            })),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("inspect Pi transcript {}", path.display()))
            }
        })
        .transpose()?
        .flatten();
    if previous == Some(&stamp) {
        return Ok(None);
    }
    if let (Some(session), Some(stamp)) = (session, &stamp) {
        let (messages, _) = transcript::page(&session.id, &stamp.path, None)?;
        let mut seen = std::collections::BTreeMap::new();
        for message in messages {
            let owner = matches!(message.author.as_str(), "user" | "owner");
            let kind = if owner {
                EntryKind::Message
            } else if message.author == "assistant" {
                EntryKind::Answer
            } else {
                continue;
            };
            let text = message
                .items
                .iter()
                .filter_map(|item| match item {
                    SessionItem::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                continue;
            }
            // Source text and timestamp are immutable. Unlike continuation tokens,
            // this identity doesn't change between subscriptions or daemon restarts.
            let hash = fingerprint(&(&message.author, &message.at, &text))?;
            let occurrence = seen.entry(hash.clone()).or_insert(0_u32);
            let id = format!("{}/replay/{hash}/{occurrence}", session.id);
            *occurrence += 1;
            let attachments = if owner {
                assistant::attachments::facts(&text)
            } else {
                Vec::new()
            };
            rows.push(ThreadEntry {
                id,
                rev: 1,
                at: message
                    .at
                    .unwrap_or_else(|| jiff::Timestamp::UNIX_EPOCH.to_string()),
                kind,
                device: None,
                via: None,
                typed_at: None,
                text: Some(text),
                state: owner.then_some(MessageState::Delivered),
                model: message.model,
                cost_usd: message
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.pointer("/cost/total"))
                    .and_then(serde_json::Value::as_f64),
                tools: Vec::new(),
                part: None,
                completion: (!owner).then_some(match message.stop_reason.as_deref() {
                    Some("aborted") => TurnCompletion::Aborted,
                    Some("error") => TurnCompletion::Failed,
                    _ => TurnCompletion::Completed,
                }),
                cost_basis: None,
                links: Vec::new(),
                card: None,
                attachments,
            });
        }
    }
    Ok(Some((
        Rows::Thread {
            scope: scope.into(),
            rows,
        },
        stamp,
    )))
}
